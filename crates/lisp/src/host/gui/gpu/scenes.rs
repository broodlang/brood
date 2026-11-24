//! 3D scenes on the device (ADR-786): the meshes `gui-mesh` uploaded, as vertex, index and
//! morph buffers; the pipelines that skin, morph and shade them (`MESH_SRC`, the WGSL twin of
//! `host::gui::mesh`'s shading model); and the targets each `[:scene …]` op of a frame renders
//! into — 4× multisampled colour with a depth buffer, resolved into a texture the textured
//! pipeline then draws where the op stands, premultiplied, like a smooth sprite.

use super::*;
use crate::host::gui::mesh::{
    LightKind, MAX_LIGHTS, MAX_SHADOW_VIEWS, Mesh, MeshBounds, Morph, Scene, ShadowPlan, Surface,
    add_morphs_packed, draw_bounds, shadow_plan, union,
};

/// Floats a vertex: position 3, normal 3, uv 2, tangent 4, joints 4 (as u32), weights 4.
const VERTEX_FLOATS: usize = 20;
/// The `SceneU` uniform (2688 bytes), padded to the dynamic-offset alignment.
const SCENE_BYTES: u64 = 2816;
/// The `DrawU` uniform, padded to the dynamic-offset alignment.
const DRAW_BYTES: u64 = 256;
const SAMPLES: u32 = 4;
const COLOR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

/// Premultiplied alpha over what is there: a blended surface (hair) onto the scene.
const PREMULTIPLIED_OVER: wgpu::BlendState = wgpu::BlendState {
    color: wgpu::BlendComponent {
        src_factor: wgpu::BlendFactor::One,
        dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
        operation: wgpu::BlendOperation::Add,
    },
    alpha: wgpu::BlendComponent {
        src_factor: wgpu::BlendFactor::One,
        dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
        operation: wgpu::BlendOperation::Add,
    },
};

/// A mesh on the device.
pub(super) struct GpuMesh {
    vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
    index_count: u32,
    vertex_count: u32,
    /// Its morph targets, sparse: a frame blends a draw's into the offsets its vertices read.
    morphs: Vec<Morph>,
    has_tangents: bool,
    skinned: bool,
    /// Where its vertices can be, to fit shadow maps as the CPU painter fits them.
    bounds: MeshBounds,
}

/// What one scene renders into, at one size.
pub(super) struct SceneTarget {
    size: (u32, u32),
    color: wgpu::TextureView,
    depth: wgpu::TextureView,
    resolve: wgpu::TextureView,
    /// The resolved picture bound for the textured pipeline.
    pub(super) composite: wgpu::BindGroup,
    /// Its shadow maps, kept while their number and size stay (ADR-788).
    shadow: Option<ShadowTarget>,
}

/// A scene's shadow maps (ADR-788): one `Depth32Float` array, a layer a shadow view, each
/// layer as big as the largest view (a smaller view renders into its top-left corner).
struct ShadowTarget {
    side: u32,
    layer_count: u32,
    /// Every layer, for the passes that render them.
    layers: Vec<wgpu::TextureView>,
    /// The whole array, for the colour pass to sample.
    array: wgpu::TextureView,
}

/// The mesh pipelines and buffers, made the first time a mesh is uploaded.
pub(super) struct MeshGpu {
    /// Indexed by `double_sided as usize * 2 + blended as usize`.
    pipelines: [wgpu::RenderPipeline; 4],
    scene_layout: wgpu::BindGroupLayout,
    /// Depth only, both faces, alpha-tested: what renders a shadow map (ADR-788).
    shadow_pipeline: wgpu::RenderPipeline,
    /// The scene group without the shadow maps, for the passes that render them.
    shadow_scene_layout: wgpu::BindGroupLayout,
    /// Compares a receiver's depth with a shadow map's (less or equal: lit).
    shadow_sampler: wgpu::Sampler,
    /// One cleared layer, bound for a scene without shadows.
    no_shadows: wgpu::TextureView,
    draw_layout: wgpu::BindGroupLayout,
    morph_layout: wgpu::BindGroupLayout,
    material_layout: wgpu::BindGroupLayout,
    scene_buffer: wgpu::Buffer,
    scene_capacity: u64,
    draw_buffer: wgpu::Buffer,
    draw_capacity: u64,
    palette_buffer: wgpu::Buffer,
    palette_capacity: u64,
    /// This frame's morph offsets: two vec4s a vertex of each morphed draw, from where its
    /// uniform says.
    morph_offset_buffer: wgpu::Buffer,
    morph_offset_capacity: u64,
    white: wgpu::TextureView,
    flat: wgpu::TextureView,
    sampler: wgpu::Sampler,
    pub(super) meshes: HashMap<u32, GpuMesh>,
    pub(super) targets: Vec<SceneTarget>,
}

fn floats_bytes(values: &[f32]) -> Vec<u8> {
    values.iter().flat_map(|v| v.to_le_bytes()).collect()
}

fn words_bytes(values: &[u32]) -> Vec<u8> {
    values.iter().flat_map(|v| v.to_le_bytes()).collect()
}

/// A buffer of at least `size` bytes (and never empty: a binding needs something).
fn buffer(
    device: &wgpu::Device,
    label: &str,
    size: u64,
    usage: wgpu::BufferUsages,
) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size: size.max(16).next_multiple_of(16),
        usage: usage | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

fn one_pixel(device: &wgpu::Device, queue: &wgpu::Queue, rgba: [u8; 4]) -> wgpu::TextureView {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("mesh default texture"),
        size: wgpu::Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        &rgba,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(4),
            rows_per_image: Some(1),
        },
        wgpu::Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
    );
    texture.create_view(&wgpu::TextureViewDescriptor::default())
}

impl MeshGpu {
    pub(super) fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> MeshGpu {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("brood gui meshes"),
            source: wgpu::ShaderSource::Wgsl(MESH_SRC.into()),
        });
        let uniform = |binding: u32, dynamic: bool| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: dynamic,
                min_binding_size: None,
            },
            count: None,
        };
        let storage = |binding: u32| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::VERTEX,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only: true },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let texture = |binding: u32| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        // the colour passes read the scene's shadow maps; the passes that render them cannot
        let scene_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("mesh scene"),
            entries: &[
                uniform(0, true),
                storage(1),
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Depth,
                        view_dimension: wgpu::TextureViewDimension::D2Array,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Comparison),
                    count: None,
                },
            ],
        });
        let shadow_scene_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("mesh shadow scene"),
                entries: &[uniform(0, true), storage(1)],
            });
        let draw_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("mesh draw"),
            entries: &[uniform(0, true)],
        });
        let morph_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("mesh morphs"),
            entries: &[storage(0)],
        });
        let material_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("mesh material"),
            entries: &[
                texture(0),
                texture(1),
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let layouts = [
            Some(&scene_layout),
            Some(&draw_layout),
            Some(&morph_layout),
            Some(&material_layout),
        ];
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("mesh"),
            bind_group_layouts: &layouts,
            immediate_size: 0,
        });
        let vertex_layout = Some(wgpu::VertexBufferLayout {
            array_stride: (VERTEX_FLOATS * 4) as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &wgpu::vertex_attr_array![
                0 => Float32x3,
                1 => Float32x3,
                2 => Float32x2,
                3 => Float32x4,
                4 => Uint32x4,
                5 => Float32x4
            ],
        });
        let make = |double_sided: bool, blended: bool| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("mesh"),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &module,
                    entry_point: Some("vs_mesh"),
                    compilation_options: Default::default(),
                    buffers: std::slice::from_ref(&vertex_layout),
                },
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    front_face: wgpu::FrontFace::Ccw,
                    cull_mode: if double_sided {
                        None
                    } else {
                        Some(wgpu::Face::Back)
                    },
                    ..Default::default()
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: DEPTH_FORMAT,
                    depth_write_enabled: Some(!blended),
                    depth_compare: Some(wgpu::CompareFunction::Less),
                    stencil: Default::default(),
                    bias: Default::default(),
                }),
                multisample: wgpu::MultisampleState {
                    count: SAMPLES,
                    ..Default::default()
                },
                fragment: Some(wgpu::FragmentState {
                    module: &module,
                    entry_point: Some("fs_mesh"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: COLOR_FORMAT,
                        blend: blended.then_some(PREMULTIPLIED_OVER),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let pipelines = [
            make(false, false),
            make(false, true),
            make(true, false),
            make(true, true),
        ];
        let shadow_layouts = [
            Some(&shadow_scene_layout),
            Some(&draw_layout),
            Some(&morph_layout),
            Some(&material_layout),
        ];
        let shadow_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("mesh shadow"),
            bind_group_layouts: &shadow_layouts,
            immediate_size: 0,
        });
        // both faces, no depth bias: the receiver is biased instead, as on the CPU painter
        let shadow_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("mesh shadow"),
            layout: Some(&shadow_layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs_shadow"),
                compilation_options: Default::default(),
                buffers: std::slice::from_ref(&vertex_layout),
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::Less),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs_shadow"),
                compilation_options: Default::default(),
                targets: &[],
            }),
            multiview_mask: None,
            cache: None,
        });
        let no_shadows = device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("no shadow maps"),
                size: wgpu::Extent3d {
                    width: 1,
                    height: 1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: DEPTH_FORMAT,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            })
            .create_view(&wgpu::TextureViewDescriptor {
                dimension: Some(wgpu::TextureViewDimension::D2Array),
                ..Default::default()
            });
        let uniforms = wgpu::BufferUsages::UNIFORM;
        let storage_usage = wgpu::BufferUsages::STORAGE;
        MeshGpu {
            pipelines,
            shadow_pipeline,
            shadow_scene_layout,
            shadow_sampler: device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("mesh shadows"),
                address_mode_u: wgpu::AddressMode::ClampToEdge,
                address_mode_v: wgpu::AddressMode::ClampToEdge,
                mag_filter: wgpu::FilterMode::Nearest,
                min_filter: wgpu::FilterMode::Nearest,
                compare: Some(wgpu::CompareFunction::LessEqual),
                ..Default::default()
            }),
            no_shadows,
            scene_buffer: buffer(device, "mesh scenes", SCENE_BYTES, uniforms),
            scene_capacity: SCENE_BYTES,
            draw_buffer: buffer(device, "mesh draws", DRAW_BYTES * 16, uniforms),
            draw_capacity: DRAW_BYTES * 16,
            palette_buffer: buffer(device, "mesh palettes", 64 * 256, storage_usage),
            palette_capacity: 64 * 256,
            morph_offset_buffer: buffer(device, "mesh morph offsets", 32 * 256, storage_usage),
            morph_offset_capacity: 32 * 256,
            scene_layout,
            draw_layout,
            morph_layout,
            material_layout,
            white: one_pixel(device, queue, [255, 255, 255, 255]),
            flat: one_pixel(device, queue, [128, 128, 255, 255]),
            sampler: device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("mesh textures"),
                address_mode_u: wgpu::AddressMode::Repeat,
                address_mode_v: wgpu::AddressMode::Repeat,
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                ..Default::default()
            }),
            meshes: HashMap::new(),
            targets: Vec::new(),
        }
    }

    /// Keep `mesh` on the device under `handle`.
    pub(super) fn upload(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        handle: u32,
        mesh: &Mesh,
    ) {
        let count = mesh.positions.len();
        // `Mesh::from_parts` keeps a mesh under the limits every device allows; a device that
        // allows less leaves the mesh undrawn here rather than failing validation on this thread
        let limits = device.limits();
        let vertex_bytes = (count * VERTEX_FLOATS * 4) as u64;
        let index_bytes = (mesh.indices.len() * 4) as u64;
        // its morph offsets, one frame's, are a storage binding of their own
        let morph_bytes = if mesh.morphs.is_empty() {
            0
        } else {
            (count * 32) as u64
        };
        if vertex_bytes.max(index_bytes) > limits.max_buffer_size
            || morph_bytes > limits.max_storage_buffer_binding_size
        {
            eprintln!(
                "brood gui: mesh {handle} is too big for this device ({vertex_bytes} vertex bytes, {morph_bytes} morph-offset bytes); it will not be drawn"
            );
            self.meshes.remove(&handle);
            return;
        }
        let mut vertices = Vec::with_capacity(count * VERTEX_FLOATS * 4);
        for index in 0..count {
            let tangent = mesh
                .tangents
                .as_ref()
                .map_or([1.0, 0.0, 0.0, 1.0], |tangents| tangents[index]);
            let (joints, weights) = if mesh.joints.is_empty() {
                ([0u16; 4], [0.0f32; 4])
            } else {
                (mesh.joints[index], mesh.weights[index])
            };
            let mut floats = Vec::with_capacity(16);
            floats.extend_from_slice(&mesh.positions[index]);
            floats.extend_from_slice(&mesh.normals[index]);
            floats.extend_from_slice(&mesh.uvs[index]);
            floats.extend_from_slice(&tangent);
            vertices.extend(floats_bytes(&floats));
            vertices.extend(words_bytes(&joints.map(u32::from)));
            vertices.extend(floats_bytes(&weights));
        }
        let vertex_buffer = buffer(
            device,
            "mesh vertices",
            vertices.len() as u64,
            wgpu::BufferUsages::VERTEX,
        );
        queue.write_buffer(&vertex_buffer, 0, &vertices);
        let indices = words_bytes(&mesh.indices);
        let index_buffer = buffer(
            device,
            "mesh indices",
            indices.len() as u64,
            wgpu::BufferUsages::INDEX,
        );
        queue.write_buffer(&index_buffer, 0, &indices);
        self.meshes.insert(
            handle,
            GpuMesh {
                vertices: vertex_buffer,
                indices: index_buffer,
                index_count: mesh.indices.len() as u32,
                vertex_count: count as u32,
                morphs: mesh.morphs.clone(),
                has_tangents: mesh.tangents.is_some(),
                skinned: !mesh.joints.is_empty(),
                bounds: mesh.bounds.clone(),
            },
        );
    }

    fn target(
        &mut self,
        device: &wgpu::Device,
        index: usize,
        size: (u32, u32),
        tex_layout: &wgpu::BindGroupLayout,
        smooth: &wgpu::Sampler,
    ) {
        if self
            .targets
            .get(index)
            .is_some_and(|target| target.size == size)
        {
            return;
        }
        let texture = |label: &str, format, samples, usage| {
            device
                .create_texture(&wgpu::TextureDescriptor {
                    label: Some(label),
                    size: wgpu::Extent3d {
                        width: size.0,
                        height: size.1,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: samples,
                    dimension: wgpu::TextureDimension::D2,
                    format,
                    usage,
                    view_formats: &[],
                })
                .create_view(&wgpu::TextureViewDescriptor::default())
        };
        let attachment = wgpu::TextureUsages::RENDER_ATTACHMENT;
        let color = texture("scene colour", COLOR_FORMAT, SAMPLES, attachment);
        let depth = texture("scene depth", DEPTH_FORMAT, SAMPLES, attachment);
        let resolve = texture(
            "scene picture",
            COLOR_FORMAT,
            1,
            attachment | wgpu::TextureUsages::TEXTURE_BINDING,
        );
        let composite = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("scene picture"),
            layout: tex_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&resolve),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(smooth),
                },
            ],
        });
        let target = SceneTarget {
            size,
            color,
            depth,
            resolve,
            composite,
            shadow: None,
        };
        if index < self.targets.len() {
            self.targets[index] = target;
        } else {
            self.targets.push(target);
        }
    }

    /// Keep scene `index`'s shadow maps as `layer_count` layers of `side`² texels, making them
    /// anew when either changed; none when the scene has no shadow this frame.
    fn shadow_target(&mut self, device: &wgpu::Device, index: usize, side: u32, layer_count: u32) {
        let target = &mut self.targets[index];
        if layer_count == 0 {
            target.shadow = None;
            return;
        }
        if target
            .shadow
            .as_ref()
            .is_some_and(|shadow| shadow.side == side && shadow.layer_count == layer_count)
        {
            return;
        }
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("scene shadow maps"),
            size: wgpu::Extent3d {
                width: side,
                height: side,
                depth_or_array_layers: layer_count,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: DEPTH_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let layers = (0..layer_count)
            .map(|layer| {
                texture.create_view(&wgpu::TextureViewDescriptor {
                    label: Some("scene shadow map"),
                    dimension: Some(wgpu::TextureViewDimension::D2),
                    base_array_layer: layer,
                    array_layer_count: Some(1),
                    ..Default::default()
                })
            })
            .collect();
        let array = texture.create_view(&wgpu::TextureViewDescriptor {
            label: Some("scene shadow maps"),
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        });
        target.shadow = Some(ShadowTarget {
            side,
            layer_count,
            layers,
            array,
        });
    }

    /// Render the frame's `scenes` into their targets (`self.targets[i]` for scene `i`), in
    /// `encoder`, before the frame's own passes draw their pictures: each scene's shadow maps
    /// first (ADR-788), planned as the CPU painter plans them, then its colour.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        scenes: &[std::sync::Arc<Scene>],
        textures: &HashMap<u32, GpuTexture>,
        tex_layout: &wgpu::BindGroupLayout,
        smooth: &wgpu::Sampler,
    ) {
        // every scene's uniform, every draw's, every palette: one buffer each, offsets apart
        let mut scene_bytes = Vec::new();
        let mut draw_bytes = Vec::new();
        let mut palette_floats: Vec<f32> = Vec::new();
        // 32 bytes a vertex of each morphed draw (`mesh::add_morphs_packed`)
        let mut morph_offsets: Vec<u8> = Vec::new();
        let morph_offset_limit = device.limits().max_storage_buffer_binding_size as usize;
        let mut plans: Vec<Vec<DrawPlan>> = Vec::new();
        let mut shadow_plans: Vec<ShadowPlan> = Vec::new();
        for scene in scenes {
            let world = scene.draws.iter().fold(None, |bounds, draw| {
                let more = self
                    .meshes
                    .get(&draw.mesh)
                    .and_then(|mesh| draw_bounds(scene, draw, &mesh.bounds));
                union(bounds, more)
            });
            let shadows = shadow_plan(scene, world);
            scene_bytes.extend(scene_uniform(scene, &shadows));
            shadow_plans.push(shadows);
            // the scene's palettes, placed after the ones before
            let mut palette_starts = Vec::with_capacity(scene.palettes.len());
            for palette in &scene.palettes {
                palette_starts.push((palette_floats.len() / 16) as u32);
                palette_floats.extend_from_slice(palette);
            }
            let mut plan = Vec::new();
            for draw in scene.draw_order() {
                let Some(mesh) = self.meshes.get(&draw.mesh) else {
                    continue;
                };
                let material = &draw.material;
                let base = material.base.and_then(|handle| textures.get(&handle));
                let normal = if mesh.has_tangents {
                    material.normal.and_then(|handle| textures.get(&handle))
                } else {
                    None
                };
                // where the draw's palette starts in the shared buffer and how many joints it holds;
                // none for a rigid draw (`Scene::palette_of`, as the CPU painter skins)
                let palette = scene.palette_of(draw).and_then(|joints| {
                    let start = palette_starts.get(draw.palette?)?;
                    Some((*start, (joints.len() / 16) as u32))
                });
                let surface = match material.surface {
                    Surface::Skin => 0u32,
                    Surface::Standard => 1,
                    Surface::Cutout => 2,
                    Surface::Blend => 3,
                };
                let premultiplied = |texture: Option<&GpuTexture>| {
                    texture.is_some_and(|texture| texture.filter == TextureFilter::Smooth)
                };
                let bits = u32::from(base.is_some())
                    | (u32::from(premultiplied(base)) << 1)
                    | (u32::from(normal.is_some()) << 2)
                    | (u32::from(premultiplied(normal)) << 3);
                // its morphs blended here, sparse, into one offset a vertex (none when no target
                // it names moves anything, or the frame's offsets would pass the binding limit)
                let vertex_count = mesh.vertex_count as usize;
                let morphed = draw.morphs.iter().any(|&(target, weight)| {
                    weight != 0.0 && (target as usize) < mesh.morphs.len()
                }) && morph_offsets.len() + vertex_count * 32 <= morph_offset_limit;
                let morph_start = (morph_offsets.len() / 32) as u32;
                if morphed {
                    let span = morph_offsets.len();
                    morph_offsets.resize(span + vertex_count * 32, 0);
                    add_morphs_packed(&mesh.morphs, &draw.morphs, &mut morph_offsets[span..]);
                }
                let mut floats = Vec::with_capacity(32);
                floats.extend_from_slice(&draw.model);
                floats.extend_from_slice(&material.color);
                floats.extend_from_slice(&[
                    material.scatter[0],
                    material.scatter[1],
                    material.scatter[2],
                    material.wrap,
                ]);
                floats.extend_from_slice(&[
                    material.roughness,
                    material.specular,
                    material.rim,
                    material.cutoff,
                ]);
                let mut bytes = floats_bytes(&floats);
                let (palette_start, palette_joints) = palette.unwrap_or((0, 0));
                let skinned = u32::from(mesh.skinned && palette_joints > 0);
                bytes.extend(words_bytes(&[surface, bits, palette_start, skinned]));
                bytes.extend(words_bytes(&[
                    u32::from(morphed),
                    mesh.vertex_count,
                    palette_joints,
                    morph_start,
                ]));
                bytes.resize(DRAW_BYTES as usize, 0);
                let at = draw_bytes.len() / DRAW_BYTES as usize;
                draw_bytes.extend(bytes);
                plan.push(DrawPlan {
                    at,
                    mesh: draw.mesh,
                    double_sided: material.double_sided,
                    blended: material.surface == Surface::Blend,
                    casts: material.cast_shadow,
                });
            }
            plans.push(plan);
        }
        // grow the buffers as the frame needs
        let grow = |device: &wgpu::Device,
                    current: &mut wgpu::Buffer,
                    capacity: &mut u64,
                    needed: u64,
                    label: &str,
                    usage| {
            if needed > *capacity {
                *capacity = needed.next_power_of_two();
                *current = buffer(device, label, *capacity, usage);
            }
        };
        grow(
            device,
            &mut self.scene_buffer,
            &mut self.scene_capacity,
            scene_bytes.len() as u64,
            "mesh scenes",
            wgpu::BufferUsages::UNIFORM,
        );
        grow(
            device,
            &mut self.draw_buffer,
            &mut self.draw_capacity,
            draw_bytes.len().max(1) as u64,
            "mesh draws",
            wgpu::BufferUsages::UNIFORM,
        );
        let palette_bytes = floats_bytes(&palette_floats);
        grow(
            device,
            &mut self.palette_buffer,
            &mut self.palette_capacity,
            palette_bytes.len().max(64) as u64,
            "mesh palettes",
            wgpu::BufferUsages::STORAGE,
        );
        grow(
            device,
            &mut self.morph_offset_buffer,
            &mut self.morph_offset_capacity,
            morph_offsets.len().max(32) as u64,
            "mesh morph offsets",
            wgpu::BufferUsages::STORAGE,
        );
        queue.write_buffer(&self.scene_buffer, 0, &scene_bytes);
        if !morph_offsets.is_empty() {
            queue.write_buffer(&self.morph_offset_buffer, 0, &morph_offsets);
        }
        if !draw_bytes.is_empty() {
            queue.write_buffer(&self.draw_buffer, 0, &draw_bytes);
        }
        if !palette_bytes.is_empty() {
            queue.write_buffer(&self.palette_buffer, 0, &palette_bytes);
        }
        for (index, scene) in scenes.iter().enumerate() {
            self.target(device, index, scene.pixel_size(), tex_layout, smooth);
            let shadows = &shadow_plans[index];
            self.shadow_target(
                device,
                index,
                shadows.layer_side,
                shadows.views.len() as u32,
            );
        }
        // a scene the frame no longer draws gives its target back
        self.targets.truncate(scenes.len());
        let scene_uniform_entry = || wgpu::BindGroupEntry {
            binding: 0,
            resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                buffer: &self.scene_buffer,
                offset: 0,
                size: std::num::NonZeroU64::new(SCENE_BYTES),
            }),
        };
        let palette_entry = || wgpu::BindGroupEntry {
            binding: 1,
            resource: self.palette_buffer.as_entire_binding(),
        };
        let shadow_scene_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("mesh shadow scene"),
            layout: &self.shadow_scene_layout,
            entries: &[scene_uniform_entry(), palette_entry()],
        });
        let draw_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("mesh draw"),
            layout: &self.draw_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &self.draw_buffer,
                    offset: 0,
                    size: std::num::NonZeroU64::new(DRAW_BYTES),
                }),
            }],
        });
        let morph_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("mesh morph offsets"),
            layout: &self.morph_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: self.morph_offset_buffer.as_entire_binding(),
            }],
        });
        for (index, scene) in scenes.iter().enumerate() {
            // the material bind groups this scene's draws sample
            let materials: Vec<wgpu::BindGroup> = scene
                .draw_order()
                .filter(|draw| self.meshes.contains_key(&draw.mesh))
                .map(|draw| {
                    let mesh = &self.meshes[&draw.mesh];
                    let view = |handle: Option<u32>| {
                        handle
                            .and_then(|handle| textures.get(&handle))
                            .map(|texture| &texture.view)
                    };
                    let base = view(draw.material.base).unwrap_or(&self.white);
                    let normal = if mesh.has_tangents {
                        view(draw.material.normal)
                    } else {
                        None
                    }
                    .unwrap_or(&self.flat);
                    device.create_bind_group(&wgpu::BindGroupDescriptor {
                        label: Some("mesh material"),
                        layout: &self.material_layout,
                        entries: &[
                            wgpu::BindGroupEntry {
                                binding: 0,
                                resource: wgpu::BindingResource::TextureView(base),
                            },
                            wgpu::BindGroupEntry {
                                binding: 1,
                                resource: wgpu::BindingResource::TextureView(normal),
                            },
                            wgpu::BindGroupEntry {
                                binding: 2,
                                resource: wgpu::BindingResource::Sampler(&self.sampler),
                            },
                        ],
                    })
                })
                .collect();
            let target = &self.targets[index];
            let scene_offset = (index as u64 * SCENE_BYTES) as u32;
            // the shadow maps: every casting draw, once a view, the view by instance index
            if let Some(shadow) = &target.shadow {
                for (view_index, (view, layer)) in shadow_plans[index]
                    .views
                    .iter()
                    .zip(&shadow.layers)
                    .enumerate()
                {
                    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("scene shadow"),
                        color_attachments: &[],
                        depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                            view: layer,
                            depth_ops: Some(wgpu::Operations {
                                load: wgpu::LoadOp::Clear(1.0),
                                store: wgpu::StoreOp::Store,
                            }),
                            stencil_ops: None,
                        }),
                        timestamp_writes: None,
                        occlusion_query_set: None,
                        multiview_mask: None,
                    });
                    let side = view.resolution as f32;
                    pass.set_viewport(0.0, 0.0, side, side, 0.0, 1.0);
                    pass.set_pipeline(&self.shadow_pipeline);
                    pass.set_bind_group(0, &shadow_scene_group, &[scene_offset]);
                    let instance = view_index as u32;
                    for (draw, material) in plans[index].iter().zip(&materials) {
                        if !draw.casts {
                            continue;
                        }
                        let mesh = &self.meshes[&draw.mesh];
                        pass.set_bind_group(
                            1,
                            &draw_group,
                            &[(draw.at as u64 * DRAW_BYTES) as u32],
                        );
                        pass.set_bind_group(2, &morph_group, &[]);
                        pass.set_bind_group(3, material, &[]);
                        pass.set_vertex_buffer(0, mesh.vertices.slice(..));
                        pass.set_index_buffer(mesh.indices.slice(..), wgpu::IndexFormat::Uint32);
                        pass.draw_indexed(0..mesh.index_count, 0, instance..instance + 1);
                    }
                }
            }
            let shadow_maps = target
                .shadow
                .as_ref()
                .map_or(&self.no_shadows, |shadow| &shadow.array);
            let scene_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("mesh scene"),
                layout: &self.scene_layout,
                entries: &[
                    scene_uniform_entry(),
                    palette_entry(),
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::TextureView(shadow_maps),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: wgpu::BindingResource::Sampler(&self.shadow_sampler),
                    },
                ],
            });
            let background = scene.background;
            let opacity = background[3].clamp(0.0, 1.0);
            let clear = wgpu::Color {
                r: (background[0].clamp(0.0, 1.0) * opacity) as f64,
                g: (background[1].clamp(0.0, 1.0) * opacity) as f64,
                b: (background[2].clamp(0.0, 1.0) * opacity) as f64,
                a: opacity as f64,
            };
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("scene"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &target.color,
                    depth_slice: None,
                    resolve_target: Some(&target.resolve),
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(clear),
                        store: wgpu::StoreOp::Discard,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &target.depth,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Discard,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_bind_group(0, &scene_group, &[scene_offset]);
            for (draw, material) in plans[index].iter().zip(&materials) {
                let mesh = &self.meshes[&draw.mesh];
                pass.set_pipeline(
                    &self.pipelines[draw.double_sided as usize * 2 + draw.blended as usize],
                );
                pass.set_bind_group(1, &draw_group, &[(draw.at as u64 * DRAW_BYTES) as u32]);
                pass.set_bind_group(2, &morph_group, &[]);
                pass.set_bind_group(3, material, &[]);
                pass.set_vertex_buffer(0, mesh.vertices.slice(..));
                pass.set_index_buffer(mesh.indices.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..mesh.index_count, 0, 0..1);
            }
        }
    }
}

/// One draw of a scene as the passes issue it.
struct DrawPlan {
    /// Its `DrawU`'s slot in the frame's draw buffer.
    at: usize,
    mesh: u32,
    double_sided: bool,
    blended: bool,
    casts: bool,
}

/// `scene`'s `SceneU` uniform (the WGSL struct in `MESH_SRC`), padded to [`SCENE_BYTES`].
fn scene_uniform(scene: &Scene, shadows: &ShadowPlan) -> Vec<u8> {
    let mut bytes = words_bytes(&scene_words(scene, shadows));
    bytes.resize(SCENE_BYTES as usize, 0);
    bytes
}

/// How many bytes [`scene_uniform`] fills before its padding, for the test that holds it to
/// the shader's `SceneU`.
#[cfg(test)]
pub(super) fn scene_uniform_len() -> usize {
    let scene = Scene {
        x: 0.0,
        y: 0.0,
        w: 1.0,
        h: 1.0,
        view_proj: [0.0; 16],
        eye: [0.0; 3],
        lights: Vec::new(),
        sky: [0.0; 3],
        ground: [0.0; 3],
        exposure: 1.0,
        background: [0.0; 4],
        palettes: Vec::new(),
        draws: Vec::new(),
    };
    scene_words(&scene, &shadow_plan(&scene, None)).len() * 4
}

/// The words of `scene`'s `SceneU`: its camera, its lights, its ambient and exposure, and the
/// shadow views `shadows` plans.
fn scene_words(scene: &Scene, shadows: &ShadowPlan) -> Vec<u32> {
    let mut words: Vec<u32> = Vec::with_capacity(SCENE_BYTES as usize / 4);
    let floats = |words: &mut Vec<u32>, values: &[f32]| {
        words.extend(values.iter().map(|value| value.to_bits()));
    };
    floats(&mut words, &scene.view_proj);
    floats(&mut words, &[scene.eye[0], scene.eye[1], scene.eye[2], 1.0]);
    for index in 0..MAX_LIGHTS {
        match scene.lights.get(index) {
            Some(light) => {
                let position = light.position;
                let direction = light.direction;
                let color = light.color;
                floats(
                    &mut words,
                    &[position[0], position[1], position[2], light.range],
                );
                floats(&mut words, &[direction[0], direction[1], direction[2], 0.0]);
                // the shadow's softness rides in the colour's spare fourth
                let softness = light.shadow.as_ref().map_or(0.0, |shadow| shadow.softness);
                floats(&mut words, &[color[0], color[1], color[2], softness]);
                let (bias, normal_bias) = light
                    .shadow
                    .as_ref()
                    .map_or((0.0, 0.0), |shadow| (shadow.bias, shadow.normal_bias));
                floats(
                    &mut words,
                    &[light.cone[0].cos(), light.cone[1].cos(), bias, normal_bias],
                );
                let kind = match light.kind {
                    LightKind::Directional => 0,
                    LightKind::Point => 1,
                    LightKind::Spot => 2,
                };
                let (first, count) = shadows.lights[index];
                words.extend_from_slice(&[kind, first, count, 0]);
            }
            None => words.extend_from_slice(&[0; 20]),
        }
    }
    floats(&mut words, &[scene.sky[0], scene.sky[1], scene.sky[2], 0.0]);
    floats(
        &mut words,
        &[scene.ground[0], scene.ground[1], scene.ground[2], 0.0],
    );
    floats(
        &mut words,
        &[
            scene.exposure,
            scene.lights.len().min(MAX_LIGHTS) as f32,
            shadows.layer_side.max(1) as f32,
            0.0,
        ],
    );
    for index in 0..MAX_SHADOW_VIEWS {
        match shadows.views.get(index) {
            Some(view) => {
                floats(&mut words, &view.view_proj);
                floats(&mut words, &[view.texel, view.resolution as f32, 0.0, 0.0]);
            }
            None => words.extend_from_slice(&[0; 20]),
        }
    }
    words
}
