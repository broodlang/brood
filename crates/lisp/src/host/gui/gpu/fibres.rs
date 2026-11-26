//! ADR-813: `:fibres`, hair strands at their real width, each fragment's coverage of its
//! pixel worked out exactly and blended, with at most three layers a pixel lit (TressFX
//! ShortCut, AMD, MIT). After a scene's colour pass:
//!
//! 1. `depth_copy`: the nearest of the scene's depth samples, to a single-sample depth.
//! 2. the fibres' layer pass (`scenes.rs`, `fs_fibre_layers`): each fragment puts its depth
//!    among its pixel's three nearest (atomics in `layers`) and multiplies the pixel's
//!    `transmit` by (1 − its coverage).
//! 3. `kth`: the third nearest depth written as depth, where there are three.
//! 4. the fibres' front pass (`fs_fibre_front`): only fragments at or before that depth are
//!    shaded (early depth test), adding colour × coverage and coverage into `accum`.
//! 5. `resolve`: the fibres' mean colour laid over the scene by (1 − `transmit`).
//!
//! The CPU painter draws `:fibres` as `:strands`; this module is the GPU painter's alone.

use super::scenes::DEPTH_FORMAT;

/// The full-screen passes' shader.
const FIBRE_SRC: &str = r#"
struct Size {
    width: u32,
    samples: u32,
    spare: vec2<u32>,
};

@group(0) @binding(0) var scene_depth: texture_depth_multisampled_2d;
@group(0) @binding(1) var<storage, read> layers: array<u32>;
@group(0) @binding(2) var transmit: texture_2d<f32>;
@group(0) @binding(3) var accum: texture_2d<f32>;
@group(0) @binding(4) var<uniform> size: Size;

@vertex
fn vs_full(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    let corner = vec2<f32>(f32((index << 1u) & 2u), f32(index & 2u));
    return vec4<f32>(corner * 2.0 - vec2<f32>(1.0), 0.0, 1.0);
}

// the nearest of the scene's depth samples at this pixel
@fragment
fn fs_depth_copy(@builtin(position) at: vec4<f32>) -> @builtin(frag_depth) f32 {
    let pixel = vec2<i32>(floor(at.xy));
    var nearest = 1.0;
    for (var sample = 0u; sample < size.samples; sample = sample + 1u) {
        nearest = min(nearest, textureLoad(scene_depth, pixel, i32(sample)));
    }
    return nearest;
}

// the third nearest fibre depth, where three fibres lie over this pixel
@fragment
fn fs_kth(@builtin(position) at: vec4<f32>) -> @builtin(frag_depth) f32 {
    let pixel = vec2<u32>(floor(at.xy));
    let stored = layers[(pixel.y * size.width + pixel.x) * 3u + 2u];
    if (stored == 0u) {
        discard;
    }
    // (its depth from the top 24 bits, the low 8 its coverage: a hair farther, so the third passes)
    return bitcast<f32>(0xFFFFFFFFu - (stored & 0xFFFFFF00u));
}

// the fibres' mean colour over the scene, by how much of the pixel they cover together
@fragment
fn fs_resolve(@builtin(position) at: vec4<f32>) -> @location(0) vec4<f32> {
    let pixel = vec2<i32>(floor(at.xy));
    let kept = textureLoad(transmit, pixel, 0).r;
    let sum = textureLoad(accum, pixel, 0);
    if (sum.a <= 1e-5) {
        return vec4<f32>(0.0);
    }
    // premultiplied, laid over the picture: its colour and its alpha both
    return vec4<f32>(sum.rgb / sum.a * (1.0 - kept), 1.0 - kept);
}
"#;

/// The transmittance target's and the colour sum's formats.
const TRANSMIT_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::R16Float;
const ACCUM_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

/// A scene target's fibre buffers, at its size.
pub(super) struct FibreTarget {
    pub(super) size: (u32, u32),
    /// The single-sample depth the fibre passes test and write.
    pub(super) depth: wgpu::TextureView,
    /// Π(1 − coverage) a pixel, cleared to 1.
    pub(super) transmit: wgpu::TextureView,
    /// Σ colour × coverage and Σ coverage of the nearest three a pixel, cleared to 0.
    pub(super) accum: wgpu::TextureView,
    /// Three u32 a pixel: the nearest fibre depths, as 0xFFFFFFFF − their bits (0: empty).
    pub(super) layers: wgpu::Buffer,
    size_buffer: wgpu::Buffer,
}

impl FibreTarget {
    pub(super) fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        size: (u32, u32),
        samples: u32,
    ) -> FibreTarget {
        let texture = |label: &str, format, usage| {
            device
                .create_texture(&wgpu::TextureDescriptor {
                    label: Some(label),
                    size: wgpu::Extent3d {
                        width: size.0,
                        height: size.1,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format,
                    usage,
                    view_formats: &[],
                })
                .create_view(&wgpu::TextureViewDescriptor::default())
        };
        let attachment = wgpu::TextureUsages::RENDER_ATTACHMENT;
        let read = wgpu::TextureUsages::TEXTURE_BINDING;
        let size_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("fibre size"),
            size: 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let words = [size.0, samples, 0, 0];
        let bytes: Vec<u8> = words.iter().flat_map(|word| word.to_le_bytes()).collect();
        queue.write_buffer(&size_buffer, 0, &bytes);
        FibreTarget {
            size,
            depth: texture("fibre depth", DEPTH_FORMAT, attachment),
            transmit: texture("fibre transmittance", TRANSMIT_FORMAT, attachment | read),
            accum: texture("fibre colour sum", ACCUM_FORMAT, attachment | read),
            layers: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("fibre layers"),
                size: u64::from(size.0) * u64::from(size.1) * 3 * 4,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
            size_buffer,
        }
    }

    /// The full-screen passes' bind group, over `scene_depth` (the scene's multisampled depth).
    pub(super) fn group(
        &self,
        device: &wgpu::Device,
        full: &FibreFullScreen,
        scene_depth: &wgpu::TextureView,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("fibre full-screen"),
            layout: &full.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(scene_depth),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: self.layers.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&self.transmit),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(&self.accum),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: self.size_buffer.as_entire_binding(),
                },
            ],
        })
    }
}

/// The full-screen pipelines: the depth copy, the third depth, the resolve.
pub(super) struct FibreFullScreen {
    layout: wgpu::BindGroupLayout,
    pub(super) depth_copy: wgpu::RenderPipeline,
    pub(super) kth: wgpu::RenderPipeline,
    pub(super) resolve: wgpu::RenderPipeline,
}

impl FibreFullScreen {
    pub(super) fn new(device: &wgpu::Device, picture_format: wgpu::TextureFormat) -> Self {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("fibre full-screen"),
            source: wgpu::ShaderSource::Wgsl(FIBRE_SRC.into()),
        });
        let fragment = wgpu::ShaderStages::FRAGMENT;
        let texture = |binding, sample_type, multisampled| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: fragment,
            ty: wgpu::BindingType::Texture {
                sample_type,
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled,
            },
            count: None,
        };
        let unfiltered = wgpu::TextureSampleType::Float { filterable: false };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("fibre full-screen"),
            entries: &[
                texture(0, wgpu::TextureSampleType::Depth, true),
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: fragment,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                texture(2, unfiltered, false),
                texture(3, unfiltered, false),
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
                    visibility: fragment,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("fibre full-screen"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let make = |entry: &str,
                    depth: Option<wgpu::DepthStencilState>,
                    target: Option<wgpu::ColorTargetState>| {
            let targets = [target];
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(entry),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &module,
                    entry_point: Some("vs_full"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: depth,
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &module,
                    entry_point: Some(entry),
                    compilation_options: Default::default(),
                    targets: if targets[0].is_some() { &targets } else { &[] },
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let write_depth = Some(wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: Some(true),
            depth_compare: Some(wgpu::CompareFunction::Always),
            stencil: Default::default(),
            bias: Default::default(),
        });
        // the fibres' colour × (1 − transmittance), premultiplied, over the picture
        let over = wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING;
        FibreFullScreen {
            depth_copy: make("fs_depth_copy", write_depth.clone(), None),
            kth: make("fs_kth", write_depth, None),
            resolve: make(
                "fs_resolve",
                None,
                Some(wgpu::ColorTargetState {
                    format: picture_format,
                    blend: Some(over),
                    write_mask: wgpu::ColorWrites::ALL,
                }),
            ),
            layout,
        }
    }
}

/// The layer pass's colour target: Π(1 − coverage) by multiplying blend.
pub(super) fn transmit_target() -> wgpu::ColorTargetState {
    wgpu::ColorTargetState {
        format: TRANSMIT_FORMAT,
        blend: Some(wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::Zero,
                dst_factor: wgpu::BlendFactor::Src,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::Zero,
                dst_factor: wgpu::BlendFactor::Src,
                operation: wgpu::BlendOperation::Add,
            },
        }),
        write_mask: wgpu::ColorWrites::ALL,
    }
}

/// The front pass's colour target: Σ colour × coverage, Σ coverage, by adding.
pub(super) fn accum_target() -> wgpu::ColorTargetState {
    let add = wgpu::BlendComponent {
        src_factor: wgpu::BlendFactor::One,
        dst_factor: wgpu::BlendFactor::One,
        operation: wgpu::BlendOperation::Add,
    };
    wgpu::ColorTargetState {
        format: ACCUM_FORMAT,
        blend: Some(wgpu::BlendState {
            color: add,
            alpha: add,
        }),
        write_mask: wgpu::ColorWrites::ALL,
    }
}
