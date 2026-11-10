//! The frame effects on the device: the lighting and post-processing pipelines with the
//! frame-sized textures they render through, the particle compute step and its emitters'
//! buffers, and the per-frame uniforms that drive them.

use super::*;

/// The uniform an emitter's compute step and its quads read (`Params` in the shaders).
pub(super) const PARTICLE_PARAMS_BYTES: u64 = 112;
/// One light instance: x, y, radius, intensity, then rgb and a pad.
pub(super) const LIGHT_FLOATS: usize = 8;
/// The `Post` uniform: twelve floats, padded to 48 bytes.
pub(super) const POST_BYTES: u64 = 48;

/// The lighting and post-processing pipelines, made the first time a frame asks for an
/// effect, and the frame-sized textures they render through.
pub(super) struct EffectsGpu {
    pub(super) light_pipeline: wgpu::RenderPipeline,
    pub(super) multiply_pipeline: wgpu::RenderPipeline,
    pub(super) bright_pipeline: wgpu::RenderPipeline,
    pub(super) blur_pipeline: wgpu::RenderPipeline,
    pub(super) final_pipeline: wgpu::RenderPipeline,
    pub(super) post_layout: wgpu::BindGroupLayout,
    pub(super) smooth_sampler: wgpu::Sampler,
    pub(super) light_instances: InstanceBuffer,
    /// One `Post` uniform per pass that reads one: bright, blur across, blur down, final.
    pub(super) post_bright: wgpu::Buffer,
    pub(super) post_across: wgpu::Buffer,
    pub(super) post_down: wgpu::Buffer,
    pub(super) post_final: wgpu::Buffer,
    pub(super) targets: Option<EffectTargets>,
}

/// The textures an effect frame renders through, at one frame size: the scene, the light
/// map, and bloom's two half-size ping-pong targets — with each pass's bind group.
pub(super) struct EffectTargets {
    pub(super) size: (u32, u32),
    pub(super) scene: wgpu::TextureView,
    pub(super) light: wgpu::TextureView,
    pub(super) bloom_a: wgpu::TextureView,
    pub(super) bloom_b: wgpu::TextureView,
    pub(super) half: (u32, u32),
    pub(super) multiply: wgpu::BindGroup,
    pub(super) bright: wgpu::BindGroup,
    pub(super) across: wgpu::BindGroup,
    pub(super) down: wgpu::BindGroup,
    pub(super) composite: wgpu::BindGroup,
}

impl EffectsGpu {
    pub(super) fn new(
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        viewport_layout: &wgpu::BindGroupLayout,
    ) -> EffectsGpu {
        let light_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("brood gui lights"),
            source: wgpu::ShaderSource::Wgsl(LIGHT_SRC.into()),
        });
        let post_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("brood gui post"),
            source: wgpu::ShaderSource::Wgsl(POST_SRC.into()),
        });
        let texture_entry = |binding: u32| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let post_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("post"),
            entries: &[
                texture_entry(0),
                texture_entry(1),
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let strip = wgpu::PrimitiveTopology::TriangleStrip;
        let list = wgpu::PrimitiveTopology::TriangleList;
        let light_pipeline = pipeline(
            device,
            "lights",
            &light_module,
            ("vs_light", "fs_light"),
            &[viewport_layout],
            Some(wgpu::VertexBufferLayout {
                array_stride: (LIGHT_FLOATS * 4) as u64,
                step_mode: wgpu::VertexStepMode::Instance,
                attributes: &wgpu::vertex_attr_array![0 => Float32x4, 1 => Float32x4],
            }),
            format,
            Some(ADD),
            strip,
        );
        let full = |label: &str, fragment: &str, blend: Option<wgpu::BlendState>| {
            pipeline(
                device,
                label,
                &post_module,
                ("vs_full", fragment),
                &[&post_layout],
                None,
                format,
                blend,
                list,
            )
        };
        let uniform = |label: &str| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: POST_BYTES,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        };
        EffectsGpu {
            light_pipeline,
            multiply_pipeline: full("light map", "fs_multiply", Some(MULTIPLY)),
            bright_pipeline: full("bloom bright pass", "fs_bright", None),
            blur_pipeline: full("bloom blur", "fs_blur", None),
            final_pipeline: full("final composite", "fs_final", None),
            smooth_sampler: device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("linear"),
                address_mode_u: wgpu::AddressMode::ClampToEdge,
                address_mode_v: wgpu::AddressMode::ClampToEdge,
                address_mode_w: wgpu::AddressMode::ClampToEdge,
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                mipmap_filter: wgpu::MipmapFilterMode::Nearest,
                ..Default::default()
            }),
            post_layout,
            light_instances: InstanceBuffer::new(device, "light instances", 4 * 1024),
            post_bright: uniform("post: bright"),
            post_across: uniform("post: blur across"),
            post_down: uniform("post: blur down"),
            post_final: uniform("post: final"),
            targets: None,
        }
    }

    /// The textures for a `w`×`h` frame, made (or remade, on a resize) as needed.
    pub(super) fn targets(
        &mut self,
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        w: u32,
        h: u32,
    ) -> &EffectTargets {
        if self.targets.as_ref().is_none_or(|t| t.size != (w, h)) {
            let view = |label: &str, w: u32, h: u32| {
                device
                    .create_texture(&wgpu::TextureDescriptor {
                        label: Some(label),
                        size: wgpu::Extent3d {
                            width: w,
                            height: h,
                            depth_or_array_layers: 1,
                        },
                        mip_level_count: 1,
                        sample_count: 1,
                        dimension: wgpu::TextureDimension::D2,
                        format,
                        usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                            | wgpu::TextureUsages::TEXTURE_BINDING,
                        view_formats: &[],
                    })
                    .create_view(&wgpu::TextureViewDescriptor::default())
            };
            let half = ((w / 2).max(1), (h / 2).max(1));
            let scene = view("scene", w, h);
            let light = view("light map", w, h);
            let bloom_a = view("bloom a", half.0, half.1);
            let bloom_b = view("bloom b", half.0, half.1);
            // the `glow` of every pass but the final one, which reads no glow: a pass may not
            // bind a texture it renders into
            let none = view("no glow", 1, 1);
            let group = |label: &str,
                         source: &wgpu::TextureView,
                         glow: &wgpu::TextureView,
                         post: &wgpu::Buffer| {
                device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some(label),
                    layout: &self.post_layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: wgpu::BindingResource::TextureView(source),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::TextureView(glow),
                        },
                        wgpu::BindGroupEntry {
                            binding: 2,
                            resource: wgpu::BindingResource::Sampler(&self.smooth_sampler),
                        },
                        wgpu::BindGroupEntry {
                            binding: 3,
                            resource: post.as_entire_binding(),
                        },
                    ],
                })
            };
            let targets = EffectTargets {
                size: (w, h),
                multiply: group("light map", &light, &none, &self.post_final),
                bright: group("bright pass", &scene, &none, &self.post_bright),
                across: group("blur across", &bloom_a, &none, &self.post_across),
                down: group("blur down", &bloom_b, &none, &self.post_down),
                composite: group("final", &scene, &bloom_a, &self.post_final),
                scene,
                light,
                bloom_a,
                bloom_b,
                half,
            };
            self.targets = Some(targets);
        }
        self.targets.as_ref().expect("made above")
    }
}

/// The particle pipelines — the compute step and the two ways of drawing a particle —
/// made the first time a frame carries an emitter, and each live emitter's buffers.
pub(super) struct ParticlesGpu {
    pub(super) step_pipeline: wgpu::ComputePipeline,
    pub(super) step_layout: wgpu::BindGroupLayout,
    pub(super) draw_layout: wgpu::BindGroupLayout,
    pub(super) blend_pipeline: wgpu::RenderPipeline,
    pub(super) add_pipeline: wgpu::RenderPipeline,
    pub(super) emitters: HashMap<u32, GpuEmitter>,
}

/// One emitter on the device: its ring of particles (`capacity` × 32 bytes, read by the
/// compute step and drawn as instances), and the `Params` uniform both read.
pub(super) struct GpuEmitter {
    pub(super) capacity: u32,
    pub(super) particles: wgpu::Buffer,
    pub(super) params: wgpu::Buffer,
    pub(super) step_group: wgpu::BindGroup,
    pub(super) draw_group: wgpu::BindGroup,
    pub(super) additive: bool,
}

impl ParticlesGpu {
    pub(super) fn new(
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        viewport_layout: &wgpu::BindGroupLayout,
    ) -> ParticlesGpu {
        let step_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("brood gui particle step"),
            source: wgpu::ShaderSource::Wgsl(PARTICLES_SRC.into()),
        });
        let draw_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("brood gui particles"),
            source: wgpu::ShaderSource::Wgsl(PARTICLE_DRAW_SRC.into()),
        });
        let uniform_entry = |visibility| wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let step_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("particle step"),
            entries: &[
                uniform_entry(wgpu::ShaderStages::COMPUTE),
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: false },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let draw_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("particle draw"),
            entries: &[uniform_entry(
                wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
            )],
        });
        let step_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("particle step"),
            bind_group_layouts: &[Some(&step_layout)],
            immediate_size: 0,
        });
        let step_pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("particle step"),
            layout: Some(&step_pipeline_layout),
            module: &step_module,
            entry_point: Some("cs_step"),
            compilation_options: Default::default(),
            cache: None,
        });
        let instances = || wgpu::VertexBufferLayout {
            array_stride: (crate::host::gui::effects::PARTICLE_FLOATS * 4) as u64,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &[
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Float32x2,
                    offset: 0,
                    shader_location: 0,
                },
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Float32x2,
                    offset: 16,
                    shader_location: 1,
                },
            ],
        };
        let draw = |label: &str, blend| {
            pipeline(
                device,
                label,
                &draw_module,
                ("vs_particle", "fs_particle"),
                &[viewport_layout, &draw_layout],
                Some(instances()),
                format,
                Some(blend),
                wgpu::PrimitiveTopology::TriangleStrip,
            )
        };
        ParticlesGpu {
            blend_pipeline: draw("particles", wgpu::BlendState::ALPHA_BLENDING),
            add_pipeline: draw("additive particles", ADD_ALPHA),
            step_pipeline,
            step_layout,
            draw_layout,
            emitters: HashMap::new(),
        }
    }

    /// Match the device's emitters to this paint's steps — a new one gets its buffers, a
    /// new capacity a new ring, a finished one is dropped — and write each step's uniform.
    pub(super) fn sync(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        steps: &[EmitterStep],
    ) {
        self.emitters
            .retain(|id, _| steps.iter().any(|step| step.id == *id));
        for step in steps {
            let capacity = step.spec.max.clamp(1, MAX_PARTICLES);
            if self
                .emitters
                .get(&step.id)
                .is_none_or(|e| e.capacity != capacity)
            {
                let emitter = self.emitter(device, capacity);
                self.emitters.insert(step.id, emitter);
            }
            let emitter = self.emitters.get_mut(&step.id).expect("inserted above");
            emitter.additive = step.spec.additive;
            queue.write_buffer(
                &emitter.params,
                0,
                as_bytes_u32(&params_words(step, capacity)),
            );
        }
    }

    pub(super) fn emitter(&self, device: &wgpu::Device, capacity: u32) -> GpuEmitter {
        // zero-initialised by wgpu: every slot starts free (`life` 0)
        let particles = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("particles"),
            size: capacity as u64 * (crate::host::gui::effects::PARTICLE_FLOATS * 4) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::VERTEX,
            mapped_at_creation: false,
        });
        let params = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("particle params"),
            size: PARTICLE_PARAMS_BYTES,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let step_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("particle step"),
            layout: &self.step_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: params.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: particles.as_entire_binding(),
                },
            ],
        });
        let draw_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("particle draw"),
            layout: &self.draw_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: params.as_entire_binding(),
            }],
        });
        GpuEmitter {
            capacity,
            particles,
            params,
            step_group,
            draw_group,
            additive: false,
        }
    }
}

/// A step as the shaders' `Params` uniform, word by word in its declared order.
pub(super) fn params_words(step: &EmitterStep, capacity: u32) -> [u32; 28] {
    let s = &step.spec;
    let f = |x: f32| x.to_bits();
    let channel = |c: [u8; 4], i: usize| f(c[i] as f32 / 255.0);
    [
        f(step.x),
        f(step.y),
        f(s.gravity[0]),
        f(s.gravity[1]),
        f(s.speed[0]),
        f(s.speed[1]),
        f(s.life[0]),
        f(s.life[1]),
        f(s.size[0]),
        f(s.size[1]),
        f(s.angle),
        f(s.spread),
        channel(s.color, 0),
        channel(s.color, 1),
        channel(s.color, 2),
        channel(s.color, 3),
        channel(s.color_end, 0),
        channel(s.color_end, 1),
        channel(s.color_end, 2),
        channel(s.color_end, 3),
        f(step.dt),
        f(s.drag),
        step.spawn_first,
        step.spawn_count,
        capacity,
        step.seed,
        u32::from(s.square),
        0,
    ]
}

impl GpuWindow {
    /// Ready the effect passes for a `w`×`h` frame: the pipelines and targets, the lights'
    /// instances, and each pass's uniform.
    pub(super) fn prepare_effects(&mut self, fx: &FrameEffects, w: u32, h: u32) {
        let format = self.config.format;
        let effects = self
            .effects
            .get_or_insert_with(|| EffectsGpu::new(&self.device, format, &self.viewport_layout));
        let half = effects.targets(&self.device, format, w, h).half;
        let lights: Vec<f32> = fx
            .lights
            .iter()
            .flat_map(|&(x, y, radius, color, intensity)| {
                [
                    x,
                    y,
                    radius,
                    intensity,
                    color[0] as f32 / 255.0,
                    color[1] as f32 / 255.0,
                    color[2] as f32 / 255.0,
                    1.0,
                ]
            })
            .collect();
        effects.light_instances.upload(
            &self.device,
            &self.queue,
            "light instances",
            as_bytes(&lights),
        );
        let (threshold, intensity, radius) = fx.bloom.unwrap_or((1.0, 0.0, 0.0));
        let (scanlines, curvature, vignette) = fx.crt.unwrap_or((0.0, 0.0, 0.0));
        // the blur runs at half size: a radius of r frame pixels is r/2 texels there
        let texels = radius / 2.0;
        let sigma = (texels / 2.0).max(0.5);
        let taps = texels.ceil().clamp(1.0, 32.0);
        let post = |step: [f32; 2], bloom: f32| {
            [
                threshold,
                (1.0 - threshold).max(1e-3),
                bloom,
                curvature,
                scanlines,
                vignette,
                step[0],
                step[1],
                sigma,
                taps,
                0.0,
                0.0,
            ]
        };
        let write = |buffer: &wgpu::Buffer, words: [f32; 12]| {
            self.queue.write_buffer(buffer, 0, as_bytes(&words));
        };
        write(&effects.post_bright, post([0.0, 0.0], 0.0));
        write(&effects.post_across, post([1.0 / half.0 as f32, 0.0], 0.0));
        write(&effects.post_down, post([0.0, 1.0 / half.1 as f32], 0.0));
        write(&effects.post_final, post([0.0, 0.0], intensity));
    }
}
