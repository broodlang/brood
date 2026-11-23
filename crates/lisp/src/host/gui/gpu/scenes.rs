//! 3D scenes on the device (ADR-786): the meshes `gui-mesh` uploaded, as vertex, index and
//! morph buffers; the pipelines that skin, morph and shade them (`MESH_SRC`, the WGSL twin of
//! `host::gui::mesh`'s shading model); and the targets each `[:scene …]` op of a frame renders
//! into — 4× multisampled colour with a depth buffer, resolved into a texture the textured
//! pipeline then draws where the op stands, premultiplied, like a smooth sprite.

use super::*;
use crate::host::gui::mesh::{MAX_LIGHTS, MAX_MORPHS, Mesh, Scene, Surface};

/// Floats a vertex: position 3, normal 3, uv 2, tangent 4, joints 4 (as u32), weights 4.
const VERTEX_FLOATS: usize = 20;
/// The `SceneU` uniform, padded to the dynamic-offset alignment.
const SCENE_BYTES: u64 = 256;
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
    morph_count: u32,
    morph_group: wgpu::BindGroup,
    has_tangents: bool,
    skinned: bool,
}

/// What one scene renders into, at one size.
pub(super) struct SceneTarget {
    size: (u32, u32),
    color: wgpu::TextureView,
    depth: wgpu::TextureView,
    resolve: wgpu::TextureView,
    /// The resolved picture bound for the textured pipeline.
    pub(super) composite: wgpu::BindGroup,
}

/// The mesh pipelines and buffers, made the first time a mesh is uploaded.
pub(super) struct MeshGpu {
    /// Indexed by `double_sided as usize * 2 + blended as usize`.
    pipelines: [wgpu::RenderPipeline; 4],
    scene_layout: wgpu::BindGroupLayout,
    draw_layout: wgpu::BindGroupLayout,
    morph_layout: wgpu::BindGroupLayout,
    material_layout: wgpu::BindGroupLayout,
    scene_buffer: wgpu::Buffer,
    scene_capacity: u64,
    draw_buffer: wgpu::Buffer,
    draw_capacity: u64,
    palette_buffer: wgpu::Buffer,
    palette_capacity: u64,
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
        let scene_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("mesh scene"),
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
        let uniforms = wgpu::BufferUsages::UNIFORM;
        let storage_usage = wgpu::BufferUsages::STORAGE;
        MeshGpu {
            pipelines,
            scene_buffer: buffer(device, "mesh scenes", SCENE_BYTES, uniforms),
            scene_capacity: SCENE_BYTES,
            draw_buffer: buffer(device, "mesh draws", DRAW_BYTES * 16, uniforms),
            draw_capacity: DRAW_BYTES * 16,
            palette_buffer: buffer(device, "mesh palettes", 64 * 256, storage_usage),
            palette_capacity: 64 * 256,
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
        let morph_bytes = (mesh.morphs.len() * count * 32) as u64;
        if vertex_bytes.max(index_bytes) > limits.max_buffer_size
            || morph_bytes > limits.max_storage_buffer_binding_size
        {
            eprintln!(
                "brood gui: mesh {handle} is too big for this device ({vertex_bytes} vertex bytes, {morph_bytes} morph bytes); it will not be drawn"
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
        // two vec4s a vertex a target: the position's offset, then the normal's
        let mut morphs = Vec::with_capacity(mesh.morphs.len() * count * 32);
        for morph in &mesh.morphs {
            for index in 0..count {
                let offset = morph.positions[index];
                let turn = morph.normals[index];
                morphs.extend(floats_bytes(&[
                    offset[0], offset[1], offset[2], 0.0, turn[0], turn[1], turn[2], 0.0,
                ]));
            }
        }
        let morph_buffer = buffer(
            device,
            "mesh morphs",
            morphs.len() as u64,
            wgpu::BufferUsages::STORAGE,
        );
        if !morphs.is_empty() {
            queue.write_buffer(&morph_buffer, 0, &morphs);
        }
        let morph_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("mesh morphs"),
            layout: &self.morph_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: morph_buffer.as_entire_binding(),
            }],
        });
        self.meshes.insert(
            handle,
            GpuMesh {
                vertices: vertex_buffer,
                indices: index_buffer,
                index_count: mesh.indices.len() as u32,
                vertex_count: count as u32,
                morph_count: mesh.morphs.len() as u32,
                morph_group,
                has_tangents: mesh.tangents.is_some(),
                skinned: !mesh.joints.is_empty(),
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
        };
        if index < self.targets.len() {
            self.targets[index] = target;
        } else {
            self.targets.push(target);
        }
    }

    /// Render the frame's `scenes` into their targets (`self.targets[i]` for scene `i`), in
    /// `encoder`, before the frame's own passes draw their pictures.
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
        let mut plans: Vec<Vec<(usize, u32, bool, bool)>> = Vec::new();
        for scene in scenes {
            let mut words = Vec::with_capacity(64);
            words.extend_from_slice(&scene.view_proj);
            words.extend_from_slice(&[scene.eye[0], scene.eye[1], scene.eye[2], 1.0]);
            for k in 0..MAX_LIGHTS {
                match scene.lights.get(k) {
                    Some(light) => {
                        words.extend_from_slice(&[
                            light.direction[0],
                            light.direction[1],
                            light.direction[2],
                            0.0,
                        ]);
                        words.extend_from_slice(&[
                            light.color[0],
                            light.color[1],
                            light.color[2],
                            0.0,
                        ]);
                    }
                    None => words.extend_from_slice(&[0.0; 8]),
                }
            }
            words.extend_from_slice(&[scene.sky[0], scene.sky[1], scene.sky[2], 0.0]);
            words.extend_from_slice(&[scene.ground[0], scene.ground[1], scene.ground[2], 0.0]);
            words.extend_from_slice(&[
                scene.exposure,
                scene.lights.len().min(MAX_LIGHTS) as f32,
                0.0,
                0.0,
            ]);
            let mut bytes = floats_bytes(&words);
            bytes.resize(SCENE_BYTES as usize, 0);
            scene_bytes.extend(bytes);
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
                let morphs: Vec<(u32, f32)> = draw
                    .morphs
                    .iter()
                    .filter(|(target, _)| *target < mesh.morph_count)
                    .take(MAX_MORPHS)
                    .copied()
                    .collect();
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
                    morphs.len() as u32,
                    mesh.vertex_count,
                    palette_joints,
                    0,
                ]));
                let mut targets = [0u32; MAX_MORPHS];
                let mut weights = [0.0f32; MAX_MORPHS];
                for (slot, (target, weight)) in morphs.iter().enumerate() {
                    targets[slot] = *target;
                    weights[slot] = *weight;
                }
                bytes.extend(words_bytes(&targets));
                bytes.extend(floats_bytes(&weights));
                bytes.resize(DRAW_BYTES as usize, 0);
                let at = draw_bytes.len() / DRAW_BYTES as usize;
                draw_bytes.extend(bytes);
                plan.push((
                    at,
                    draw.mesh,
                    material.double_sided,
                    material.surface == Surface::Blend,
                ));
                let _ = (base, normal);
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
        queue.write_buffer(&self.scene_buffer, 0, &scene_bytes);
        if !draw_bytes.is_empty() {
            queue.write_buffer(&self.draw_buffer, 0, &draw_bytes);
        }
        if !palette_bytes.is_empty() {
            queue.write_buffer(&self.palette_buffer, 0, &palette_bytes);
        }
        let scene_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("mesh scene"),
            layout: &self.scene_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer: &self.scene_buffer,
                        offset: 0,
                        size: std::num::NonZeroU64::new(SCENE_BYTES),
                    }),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: self.palette_buffer.as_entire_binding(),
                },
            ],
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
        for (index, scene) in scenes.iter().enumerate() {
            self.target(device, index, scene.pixel_size(), tex_layout, smooth);
        }
        // a scene the frame no longer draws gives its target back
        self.targets.truncate(scenes.len());
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
            pass.set_bind_group(0, &scene_group, &[(index as u64 * SCENE_BYTES) as u32]);
            for ((at, handle, double_sided, blended), material) in
                plans[index].iter().zip(&materials)
            {
                let mesh = &self.meshes[handle];
                pass.set_pipeline(&self.pipelines[*double_sided as usize * 2 + *blended as usize]);
                pass.set_bind_group(1, &draw_group, &[(*at as u64 * DRAW_BYTES) as u32]);
                pass.set_bind_group(2, &mesh.morph_group, &[]);
                pass.set_bind_group(3, material, &[]);
                pass.set_vertex_buffer(0, mesh.vertices.slice(..));
                pass.set_index_buffer(mesh.indices.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..mesh.index_count, 0, 0..1);
            }
        }
    }
}
