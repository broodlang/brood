//! 3D scenes on the device (ADR-786): the meshes `gui-mesh` uploaded, as vertex, index and
//! morph buffers; the pipelines that skin, morph and shade them (`MESH_SRC`, the WGSL twin of
//! `host::gui::mesh`'s shading model); and the targets each `[:scene …]` op of a frame renders
//! into — 4× multisampled colour with a depth buffer, resolved into a texture the textured
//! pipeline then draws where the op stands, premultiplied, like a smooth sprite.

use super::*;
use crate::host::gui::mesh::{
    Bounds, EmitterPlan, LightKind, MAX_EMITTERS, MAX_EYES, MAX_LIGHTS, MAX_SHADOW_VIEWS, Mesh,
    MeshBounds, Morph, SKIN_TABLE_HEIGHT, SKIN_TABLE_WIDTH, Scene, ShadowArrays, ShadowPlan,
    Surface, add_morphs_packed, draw_bounds, emitter_plan, shadow_plan, skin_table, transform,
    union,
};

/// Floats a vertex: position 3, normal 3, uv 2, tangent 4, joints 4 (as u32), weights 4,
/// curvature 1.
pub(super) const VERTEX_FLOATS: usize = 28;

/// A `:fibres` mesh casts its shadow from one strand in this many (its triangles ordered by each
/// strand's random number, so the first share is an even choice), each drawn this many times as
/// wide, so the shadow covers as much (ADR-813): the shadow pass was most of a head's cost.
pub(super) const FIBRE_SHADOW_SHARE: u32 = 8;
/// The side of a scene's hair opacity maps (ADR-827), one layer a shadow view whatever its own
/// resolution: hair's density through the light is smooth, so it needs fewer texels than a
/// depth does. Its four slices behind the nearest hair are `HAIR_SLICES` in the shader.
pub(super) const HAIR_OPACITY_SIDE: u32 = 1024;
const HAIR_OPACITY_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
/// The `SceneU` uniform (4128 bytes: each shadow view says where its map is kept, ADR-800, and
/// where its hair can be, ADR-827), padded to the dynamic-offset alignment.
pub(super) const SCENE_BYTES: u64 = 4352;
/// The `DrawU` uniform (336 bytes with an `:eye` surface's optics, ADR-796, and a hair's `:sheen`, ADR-799), padded to the
/// dynamic-offset alignment.
pub(super) const DRAW_BYTES: u64 = 512;
/// The samples a pixel the scene is drawn with where the adapter cannot do eight (ADR-799).
const FALLBACK_SAMPLES: u32 = 4;
const COLOR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
pub(super) const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

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
    /// Its vertices turn about their centres of rotation (ADR-814): it has `:centres`.
    turning: bool,
    /// Where its vertices can be, to fit shadow maps as the CPU painter fits them.
    bounds: MeshBounds,
    /// Its softness a vertex (ADR-805), when it has one.
    softness: Option<wgpu::Buffer>,
    /// For a mesh that can be pressed: what its squeezed skin is relaxed by (ADR-822).
    ring: Option<MeshRing>,
}

/// How a pressed mesh's skin is relaxed where skinning squeezes it (ADR-822): its vertices
/// welded into points, each point's ring of neighbours, and where it sits against them at rest.
pub(super) struct MeshRing {
    /// Its indices, u32: the point and vertex counts, then each vertex's point (its weld), each
    /// point's first vertex (where its posed place is read), each point's ring as a span (`points
    /// + 1` offsets) and the rings themselves (point indices). One buffer, as the contact group
    /// may hold only eight storage buffers.
    index: wgpu::Buffer,
    /// Its vec4s: each point at rest (its offset from its ring's centroid along its normal and
    /// across its frame's two tangents, and its ring's mean edge length), each point's rest place,
    /// then the frame's work, three a point (as posed, w its squeeze mask; then two halves to relax between).
    data: wgpu::Buffer,
    points: u32,
}

/// How many relax passes a frame (ADR-822), each moving a squeezed point `RELAX_STEP` of the way: each
/// reaches one ring further, so the 4 cm waves on her shoulder tops (a mesh of 3–4 mm) need tens; 48
/// of 16, 48 and 96 filmed side by side (c8's pick for the user, 2026-11-27).
pub(super) const RELAX_ITERATIONS: u32 = 48;

/// A pressed mesh's relax data (ADR-822), built once from its rest shape.
fn mesh_ring(device: &wgpu::Device, queue: &wgpu::Queue, mesh: &Mesh) -> Option<MeshRing> {
    let mut points: std::collections::HashMap<[i64; 3], u32> = std::collections::HashMap::new();
    let mut reps: Vec<u32> = Vec::new();
    let welds: Vec<u32> = mesh
        .positions
        .iter()
        .enumerate()
        .map(|(vertex, position)| {
            let key = position.map(|axis| (axis as f64 * 1.0e6).round() as i64);
            *points.entry(key).or_insert_with(|| {
                reps.push(vertex as u32);
                reps.len() as u32 - 1
            })
        })
        .collect();
    let count = reps.len();
    if count == 0 {
        return None;
    }
    let mut neighbours: Vec<Vec<u32>> = vec![Vec::new(); count];
    for triangle in mesh.indices.as_chunks::<3>().0 {
        let corners = triangle.map(|index| welds[index as usize]);
        for slot in 0..3 {
            let point = corners[slot] as usize;
            for other in [corners[(slot + 1) % 3], corners[(slot + 2) % 3]] {
                if other as usize != point && !neighbours[point].contains(&other) {
                    neighbours[point].push(other);
                }
            }
        }
    }
    let unit = |v: [f32; 3]| {
        let length = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
        if length < 1.0e-20 {
            [0.0, 0.0, 0.0]
        } else {
            v.map(|axis| axis / length)
        }
    };
    let at = |point: usize| mesh.positions[reps[point] as usize];
    let mut offsets = Vec::with_capacity(count + 1);
    let mut ring = Vec::new();
    let mut rest = Vec::with_capacity(count * 4);
    for point in 0..count {
        offsets.push(ring.len() as u32);
        ring.extend_from_slice(&neighbours[point]);
        let here = at(point);
        // (the vertex's own normal, as the posed frame takes the normal its bones carry)
        let normal = unit(mesh.normals[reps[point] as usize]);
        let near = &neighbours[point];
        if near.is_empty() {
            rest.extend_from_slice(&[0.0, 0.0, 0.0, 0.0]);
            continue;
        }
        let mut centroid = [0.0f32; 3];
        let mut edges = 0.0f32;
        for &other in near {
            let there = at(other as usize);
            for axis in 0..3 {
                centroid[axis] += there[axis] / near.len() as f32;
            }
            let apart = [0, 1, 2].map(|axis| there[axis] - here[axis]);
            edges += (apart[0] * apart[0] + apart[1] * apart[1] + apart[2] * apart[2]).sqrt();
        }
        // (its frame: the normal, the first neighbour's direction laid in the tangent plane, and
        // their cross; the shader builds the same frame from the posed surface)
        let first = at(near[0] as usize);
        let toward = [0, 1, 2].map(|axis| first[axis] - here[axis]);
        let lift = toward[0] * normal[0] + toward[1] * normal[1] + toward[2] * normal[2];
        let tangent = unit([0, 1, 2].map(|axis| toward[axis] - normal[axis] * lift));
        let other_tangent = [
            normal[1] * tangent[2] - normal[2] * tangent[1],
            normal[2] * tangent[0] - normal[0] * tangent[2],
            normal[0] * tangent[1] - normal[1] * tangent[0],
        ];
        let offset = [0, 1, 2].map(|axis| here[axis] - centroid[axis]);
        let dot = |a: [f32; 3], b: [f32; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
        rest.extend_from_slice(&[
            dot(offset, normal),
            dot(offset, tangent),
            dot(offset, other_tangent),
            edges / near.len() as f32,
        ]);
    }
    offsets.push(ring.len() as u32);
    let store = |label: &str, bytes: &[u8]| {
        let made = buffer(
            device,
            label,
            bytes.len().max(16) as u64,
            wgpu::BufferUsages::STORAGE,
        );
        queue.write_buffer(&made, 0, bytes);
        made
    };
    let words = |values: &[u32]| {
        values
            .iter()
            .flat_map(|value| value.to_le_bytes())
            .collect::<Vec<u8>>()
    };
    let mut index = vec![count as u32, welds.len() as u32];
    index.extend_from_slice(&welds);
    index.extend_from_slice(&reps);
    index.extend_from_slice(&offsets);
    index.extend_from_slice(&ring);
    // (the rest, then room for the frame's work: three vec4 a point, written each frame)
    // (the rest offsets, then each point's rest place (for each ring edge's own rest length), then
    // room for the frame's work: three vec4 a point, written each frame)
    let mut data = rest;
    for point in 0..count {
        let here = at(point);
        data.extend_from_slice(&[here[0], here[1], here[2], 0.0]);
    }
    data.resize(count * 5 * 4, 0.0);
    Some(MeshRing {
        index: store("mesh ring index", &words(&index)),
        data: store("mesh ring data", &floats_bytes(&data)),
        points: count as u32,
    })
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
    /// Its contact buffers (ADR-805), while it presses any skin.
    contacts: Option<ContactTarget>,
    /// Its fibre buffers (ADR-813), made the first frame it has `:fibres` and kept at its size.
    fibres: Option<super::fibres::FibreTarget>,
}

/// A scene's contact buffers (ADR-805): its pressed vertices posed (three vec4 each: world
/// position, normal, tangent), its contacts as the shader reads them, each one's deepest press.
struct ContactTarget {
    posed: wgpu::Buffer,
    /// How many vertices `posed` holds room for.
    capacity: u32,
    contacts: wgpu::Buffer,
    depths: wgpu::Buffer,
}

/// A scene's `:contacts` as the shader's `ContactList` holds them (ADR-805): how many, then six
/// vec4 each (the third its `:drag`, the fourth its `:skip` joints, the fifth its `:only`, the sixth its `:within`), `[x y z kind]` and `[x y z w]`: a plane's normal and offset (kind 0), a sphere's
/// centre and radius (1), a capsule's start, end and radius (2).
fn contact_words(scene: &Scene) -> Vec<u8> {
    use crate::host::gui::mesh::{Contact, MAX_CONTACTS};
    let contacts = &scene.contacts[..scene.contacts.len().min(MAX_CONTACTS)];
    let mut bytes = words_bytes(&[contacts.len() as u32, 0, 0, 0]);
    let mut floats = vec![0.0f32; MAX_CONTACTS * 24];
    for (slot, contact) in contacts.iter().enumerate() {
        let at = slot * 24;
        let [first, second] = match *contact {
            Contact::Plane { normal, offset } => [
                [normal[0], normal[1], normal[2], 0.0],
                [offset, 0.0, 0.0, 0.0],
            ],
            Contact::Sphere { centre, radius } => [
                [centre[0], centre[1], centre[2], 1.0],
                [0.0, 0.0, 0.0, radius],
            ],
            Contact::Capsule { start, end, radius } => [
                [start[0], start[1], start[2], 2.0],
                [end[0], end[1], end[2], radius],
            ],
        };
        floats[at..at + 4].copy_from_slice(&first);
        floats[at + 4..at + 8].copy_from_slice(&second);
        // the third: how far it drags the skin it presses along its surface
        if let Some(drag) = scene.contact_drags.get(slot) {
            floats[at + 8..at + 11].copy_from_slice(drag);
        }
        // the fourth: its own limb's joints, whose skin it never presses (-1: none)
        let skip = scene
            .contact_skips
            .get(slot)
            .copied()
            .unwrap_or([u16::MAX; 4]);
        for (offset, joint) in skip.iter().enumerate() {
            floats[at + 12 + offset] = if *joint == u16::MAX {
                -1.0
            } else {
                f32::from(*joint)
            };
        }
        // the fifth: the one limb whose skin alone it presses (-1: any)
        let only = scene
            .contact_onlys
            .get(slot)
            .copied()
            .unwrap_or([u16::MAX; 4]);
        for (offset, joint) in only.iter().enumerate() {
            floats[at + 16 + offset] = if *joint == u16::MAX {
                -1.0
            } else {
                f32::from(*joint)
            };
        }
        // the sixth: the side of a plane it acts on (`:within`, ADR-816; all zero: everywhere)
        if let Some(within) = scene.contact_withins.get(slot) {
            floats[at + 20..at + 24].copy_from_slice(within);
        }
    }
    bytes.extend(floats_bytes(&floats));
    bytes
}

/// A scene's shadow maps (ADR-788, ADR-800): two `Depth32Float` arrays, a layer a shadow
/// view — array 0 at the largest view's size, array 1 at the largest of the smaller views'
/// ([`ShadowArrays`]); a view smaller than its array's layers renders into its top-left
/// corner. Array 1 is absent when every view is of the largest size.
struct ShadowTarget {
    arrays: ShadowArrays,
    /// Each array's layers, for the passes that render them.
    layers: [Vec<wgpu::TextureView>; 2],
    /// Each whole array, for the colour pass to sample (none: array 1 has no layers).
    whole: [Option<wgpu::TextureView>; 2],
    /// The hair's opacity maps (ADR-827), a layer a view by its index, at `HAIR_OPACITY_SIDE`:
    /// the nearest hair's depth, and the hair's density summed in four slices behind it. Each
    /// view's layers for the passes that render them, and each whole array to read.
    views: u32,
    opacity_front_layers: Vec<wgpu::TextureView>,
    opacity_layers: Vec<wgpu::TextureView>,
    opacity_front: wgpu::TextureView,
    opacity: wgpu::TextureView,
}

/// The mesh pipelines and buffers, made the first time a mesh is uploaded.
pub(super) struct MeshGpu {
    /// The samples a pixel its scenes are drawn with (`scene_samples`, ADR-799).
    samples: u32,
    /// Indexed by `double_sided as usize * 2 + blended as usize`.
    pipelines: [wgpu::RenderPipeline; 4],
    /// The hair's depth pass and its front pass, one-sided and double-sided (ADR-802).
    hair_depth_pipelines: [wgpu::RenderPipeline; 2],
    hair_front_pipelines: [wgpu::RenderPipeline; 2],
    /// The fibres' scene group layout, their layer and front passes, and their full-screen passes
    /// (ADR-813: `fibres.rs`).
    fibre_scene_layout: wgpu::BindGroupLayout,
    fibre_layers_pipelines: [wgpu::RenderPipeline; 2],
    fibre_front_pipelines: [wgpu::RenderPipeline; 2],
    fibre_full: super::fibres::FibreFullScreen,
    scene_layout: wgpu::BindGroupLayout,
    /// Depth only, both faces, alpha-tested: what renders a shadow map (ADR-788), of what is
    /// opaque (hair has its own maps).
    shadow_pipeline: wgpu::RenderPipeline,
    /// The hair's opacity maps (ADR-827): its nearest depth, then its density in slices behind
    /// it (its own scene group: the nearest depth read); and the empty ones bound where a scene
    /// has none.
    opacity_front_pipeline: wgpu::RenderPipeline,
    opacity_pipeline: wgpu::RenderPipeline,
    opacity_scene_layout: wgpu::BindGroupLayout,
    no_opacity_front: wgpu::TextureView,
    no_opacity: wgpu::TextureView,
    /// The contact passes' layouts and pipelines (ADR-805), and the posed buffer bound where a
    /// scene presses nothing.
    contact_morph_layout: wgpu::BindGroupLayout,
    contact_layout: wgpu::BindGroupLayout,
    contact_depth_pipeline: wgpu::ComputePipeline,
    contact_deform_pipeline: wgpu::ComputePipeline,
    /// The relax passes (ADR-822): gather, mask, even, odd, scatter.
    contact_relax_pipelines: [wgpu::ComputePipeline; 5],
    no_ring: wgpu::Buffer,
    no_ring_work: wgpu::Buffer,
    no_posed: wgpu::Buffer,
    /// The scene group without the shadow maps, for the passes that render them.
    shadow_scene_layout: wgpu::BindGroupLayout,
    /// Compares a receiver's depth with a shadow map's (less or equal: lit).
    shadow_sampler: wgpu::Sampler,
    /// One cleared layer, bound for a scene without shadows, and for array 1 when a scene's
    /// maps are all in array 0 (ADR-800).
    no_shadows: wgpu::TextureView,
    /// `mesh::skin_table` as the colour passes read it (ADR-794): `Rgba32Float`, the CPU
    /// painter's own numbers.
    skin_table: wgpu::TextureView,
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
    /// The scene passes' timestamps where the device has them (ADR-801).
    pub(super) timer: Option<super::scene_timer::SceneTimer>,
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

/// `mesh::skin_table` on the device: one `Rgba32Float` texel a table entry, row by row, so the
/// shader's `skin_diffuse` reads the numbers the CPU painter's does.
fn skin_table_texture(device: &wgpu::Device, queue: &wgpu::Queue) -> wgpu::TextureView {
    let size = wgpu::Extent3d {
        width: SKIN_TABLE_WIDTH as u32,
        height: SKIN_TABLE_HEIGHT as u32,
        depth_or_array_layers: 1,
    };
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("mesh skin table"),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba32Float,
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
        &skin_table_bytes(),
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(SKIN_TABLE_WIDTH as u32 * 16),
            rows_per_image: Some(SKIN_TABLE_HEIGHT as u32),
        },
        size,
    );
    texture.create_view(&wgpu::TextureViewDescriptor::default())
}

/// The skin table as the bytes of its texture: red, green, blue and 1, four floats a texel.
pub(super) fn skin_table_bytes() -> Vec<u8> {
    skin_table()
        .iter()
        .flat_map(|texel| [texel[0], texel[1], texel[2], 1.0])
        .flat_map(f32::to_le_bytes)
        .collect()
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

/// Whether a scene's hair is drawn in two passes, its coverage to depth and then its colour
/// where that depth is its own (ADR-802): on by default (on the game that asked, it cut the
/// scene from 51 to 8 ms on the card); `BROOD_GUI_HAIR_PREPASS=0` draws it in one pass, to
/// compare.
pub(super) fn hair_prepass() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("BROOD_GUI_HAIR_PREPASS").as_deref() != Ok("0"))
}

/// A trial of what a scene's hair costs on the card, for measuring only
/// (`BROOD_GUI_HAIR_TRIAL`): 0 drawn as ever (the default), 1 its coverage alone, lit by
/// nothing, 2 without its hair-on-hair shadow. The shader's `HAIR_TRIAL`.
pub(super) fn hair_trial() -> u32 {
    std::env::var("BROOD_GUI_HAIR_TRIAL")
        .ok()
        .and_then(|text| text.trim().parse::<u32>().ok())
        .filter(|trial| *trial <= 2)
        .unwrap_or(0)
}

/// The samples a pixel scenes are drawn with on `adapter`'s `device`: four; eight when asked
/// (`BROOD_GUI_SAMPLES=8`) and both the scene's colour and depth formats can be multisampled 8×
/// (the device asked for the adapter's own format features), so a `:hair` or `:strands`
/// surface's coverage has nine levels, not five (ADR-799). Four is the default: on the game
/// that asked for eight, its figure's hair looked the same at eight, and its cost could not
/// be measured cleanly.
pub(super) fn scene_samples(adapter: &wgpu::Adapter, device: &wgpu::Device) -> u32 {
    if std::env::var("BROOD_GUI_SAMPLES").as_deref() != Ok("8")
        || !device
            .features()
            .contains(wgpu::Features::TEXTURE_ADAPTER_SPECIFIC_FORMAT_FEATURES)
    {
        return FALLBACK_SAMPLES;
    }
    let eight = |format| {
        adapter
            .get_texture_format_features(format)
            .flags
            .sample_count_supported(8)
    };
    if eight(COLOR_FORMAT) && eight(DEPTH_FORMAT) {
        8
    } else {
        FALLBACK_SAMPLES
    }
}

impl MeshGpu {
    pub(super) fn new(device: &wgpu::Device, queue: &wgpu::Queue, samples: u32) -> MeshGpu {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("brood gui meshes"),
            source: wgpu::ShaderSource::Wgsl(MESH_SRC.into()),
        });
        let uniform = |binding: u32, dynamic: bool| wgpu::BindGroupLayoutEntry {
            binding,
            // (the contact passes pose vertices in compute, ADR-805)
            visibility: wgpu::ShaderStages::VERTEX_FRAGMENT | wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: dynamic,
                min_binding_size: None,
            },
            count: None,
        };
        let storage = |binding: u32| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::COMPUTE,
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
        // a shadow map array (ADR-788): the largest maps at binding 2, the smaller at 5 (ADR-800)
        let shadow_maps = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Depth,
                view_dimension: wgpu::TextureViewDimension::D2Array,
                multisampled: false,
            },
            count: None,
        };
        // the colour passes read the scene's shadow maps; the passes that render them cannot
        let scene_entries = [
            uniform(0, true),
            storage(1),
            shadow_maps(2),
            wgpu::BindGroupLayoutEntry {
                binding: 3,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Comparison),
                count: None,
            },
            // the skin table, loaded texel by texel: no sampler, so not filterable
            wgpu::BindGroupLayoutEntry {
                binding: 4,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: false },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            shadow_maps(5),
            // the hair's opacity maps (ADR-827): its nearest depth, and its density in four
            // slices behind it (loaded texel by texel: not filterable)
            shadow_maps(7),
            wgpu::BindGroupLayoutEntry {
                binding: 8,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: false },
                    view_dimension: wgpu::TextureViewDimension::D2Array,
                    multisampled: false,
                },
                count: None,
            },
        ];
        // the hair opacity pass's (ADR-827): the shadow passes' scene group, and the hair's
        // nearest depth it measures each fragment from
        let opacity_scene_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("hair opacity scene"),
                entries: &[uniform(0, true), storage(1), shadow_maps(7)],
            });
        let scene_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("mesh scene"),
            entries: &scene_entries,
        });
        // the fibres' scene group (ADR-813): the scene's, and the pixels' three nearest fibre depths
        let mut fibre_scene_entries = scene_entries.to_vec();
        fibre_scene_entries.push(wgpu::BindGroupLayoutEntry {
            binding: 6,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only: false },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        });
        let fibre_scene_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("fibre scene"),
                entries: &fibre_scene_entries,
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
        // the morph offsets, and the scene's posed vertices that a pressed draw reads (ADR-805)
        let morph_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("mesh morphs"),
            entries: &[storage(0), storage(1)],
        });
        // the contact passes' own (ADR-805): the morph offsets alone (they write the posed vertices),
        // and the mesh's vertices, the contacts, their depths, the posed vertices, the softness
        let contact_morph_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("contact morphs"),
                entries: &[storage(0)],
            });
        let compute_storage = |binding: u32, read_only: bool| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let contact_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("contacts"),
            entries: &[
                compute_storage(8, true),
                wgpu::BindGroupLayoutEntry {
                    binding: 9,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                compute_storage(10, false),
                compute_storage(11, false),
                compute_storage(12, true),
                // (its squeezed skin relaxed, ADR-822: its ring index and its rest and work)
                compute_storage(17, true),
                compute_storage(18, false),
            ],
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
                texture(3),
                // the detail tile (ADR-797), through the same repeating sampler
                texture(4),
                // a skin's regions map (ADR-826)
                texture(5),
                // the expression wrinkles (ADR-817): the compress and stretch maps, the two zone masks
                // (13 on: the contact passes hold 8 … 12 of this group in the same module)
                texture(13),
                texture(14),
                texture(15),
                texture(16),
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
                5 => Float32x4,
                6 => Float32,
                7 => Float32x3,
                8 => Float32x2,
                9 => Float32,
                10 => Float32
            ],
        });
        // a colour pipeline: its faces, whether it blends, and its fragment entry, depth state and
        // colour writes (the hair's depth and front passes differ there, ADR-802)
        let make_with = |double_sided: bool,
                         blended: bool,
                         entry: &str,
                         depth_write: bool,
                         depth_compare: wgpu::CompareFunction,
                         write_mask: wgpu::ColorWrites| {
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
                    depth_write_enabled: Some(depth_write),
                    depth_compare: Some(depth_compare),
                    stencil: Default::default(),
                    bias: Default::default(),
                }),
                multisample: wgpu::MultisampleState {
                    count: samples,
                    ..Default::default()
                },
                fragment: Some(wgpu::FragmentState {
                    module: &module,
                    entry_point: Some(entry),
                    // the shader's `SAMPLES`: how many bits of a hair's coverage mask it fills
                    compilation_options: wgpu::PipelineCompilationOptions {
                        constants: &[
                            ("SAMPLES", f64::from(samples)),
                            ("HAIR_TRIAL", f64::from(hair_trial())),
                        ],
                        ..Default::default()
                    },
                    targets: &[Some(wgpu::ColorTargetState {
                        format: COLOR_FORMAT,
                        blend: blended.then_some(PREMULTIPLIED_OVER),
                        write_mask,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let make = |double_sided: bool, blended: bool| {
            make_with(
                double_sided,
                blended,
                "fs_mesh",
                !blended,
                wgpu::CompareFunction::Less,
                wgpu::ColorWrites::ALL,
            )
        };
        // the hair's two passes, one-sided and double-sided (ADR-802): its coverage to depth alone,
        // then its colour where that depth is its own
        let hair_depth_pipelines = [false, true].map(|double_sided| {
            make_with(
                double_sided,
                false,
                "fs_hair_depth",
                true,
                wgpu::CompareFunction::Less,
                wgpu::ColorWrites::empty(),
            )
        });
        let hair_front_pipelines = [false, true].map(|double_sided| {
            make_with(
                double_sided,
                false,
                "fs_hair_front",
                false,
                wgpu::CompareFunction::Equal,
                wgpu::ColorWrites::ALL,
            )
        });
        // the fibres' two passes (ADR-813, `fibres.rs`), one-sided and double-sided: single-sample,
        // against the fibres' own depth, never writing it
        let fibre_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("fibre"),
            bind_group_layouts: &[
                Some(&fibre_scene_layout),
                Some(&draw_layout),
                Some(&morph_layout),
                Some(&material_layout),
            ],
            immediate_size: 0,
        });
        let make_fibre = |double_sided: bool,
                          entry: &str,
                          depth_compare: wgpu::CompareFunction,
                          target: wgpu::ColorTargetState| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(entry),
                layout: Some(&fibre_layout),
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
                    depth_write_enabled: Some(false),
                    depth_compare: Some(depth_compare),
                    stencil: Default::default(),
                    bias: Default::default(),
                }),
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &module,
                    entry_point: Some(entry),
                    compilation_options: wgpu::PipelineCompilationOptions {
                        constants: &[
                            ("SAMPLES", f64::from(samples)),
                            ("HAIR_TRIAL", f64::from(hair_trial())),
                        ],
                        ..Default::default()
                    },
                    targets: &[Some(target)],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let fibre_layers_pipelines = [false, true].map(|double_sided| {
            make_fibre(
                double_sided,
                "fs_fibre_layers",
                wgpu::CompareFunction::Less,
                super::fibres::transmit_target(),
            )
        });
        let fibre_front_pipelines = [false, true].map(|double_sided| {
            make_fibre(
                double_sided,
                "fs_fibre_front",
                wgpu::CompareFunction::LessEqual,
                super::fibres::accum_target(),
            )
        });
        let fibre_full = super::fibres::FibreFullScreen::new(device, COLOR_FORMAT);
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
        // the hair's opacity maps (ADR-827): its nearest depth, every fragment kept, then its
        // density, each fragment added into the slices behind that depth it falls before
        let opacity_front_pipeline =
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("hair nearest depth"),
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
                    entry_point: Some("fs_hair_opacity_front"),
                    compilation_options: Default::default(),
                    targets: &[],
                }),
                multiview_mask: None,
                cache: None,
            });
        let opacity_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("hair opacity"),
            bind_group_layouts: &[
                Some(&opacity_scene_layout),
                Some(&draw_layout),
                Some(&morph_layout),
                Some(&material_layout),
            ],
            immediate_size: 0,
        });
        let adding = wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::One,
            dst_factor: wgpu::BlendFactor::One,
            operation: wgpu::BlendOperation::Add,
        };
        let opacity_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("hair opacity"),
            layout: Some(&opacity_layout),
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
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs_hair_opacity"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: HAIR_OPACITY_FORMAT,
                    blend: Some(wgpu::BlendState {
                        color: adding,
                        alpha: adding,
                    }),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        // the hair's opacity maps bound where a scene has none: one layer each (a nearest depth
        // of 0 and no density: nothing in the way)
        let empty_array = |label, format| {
            device
                .create_texture(&wgpu::TextureDescriptor {
                    label: Some(label),
                    size: wgpu::Extent3d {
                        width: 1,
                        height: 1,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                })
                .create_view(&wgpu::TextureViewDescriptor {
                    dimension: Some(wgpu::TextureViewDimension::D2Array),
                    ..Default::default()
                })
        };
        let no_opacity_front = empty_array("no hair nearest depth", DEPTH_FORMAT);
        let no_opacity = empty_array("no hair opacity", HAIR_OPACITY_FORMAT);
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
        // the contact passes (ADR-805): each pressed vertex posed once, each contact's deepest press
        // found, then the vertices pressed; the vertex stages read them
        let contact_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("contacts"),
                bind_group_layouts: &[
                    Some(&shadow_scene_layout),
                    Some(&draw_layout),
                    Some(&contact_morph_layout),
                    Some(&contact_layout),
                ],
                immediate_size: 0,
            });
        let contact_pipeline = |entry: &str| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(entry),
                layout: Some(&contact_pipeline_layout),
                module: &module,
                entry_point: Some(entry),
                compilation_options: Default::default(),
                cache: None,
            })
        };
        let contact_depth_pipeline = contact_pipeline("cs_contact_depth");
        let contact_deform_pipeline = contact_pipeline("cs_contact_deform");
        let contact_relax_pipelines = [
            contact_pipeline("cs_relax_gather"),
            contact_pipeline("cs_relax_mask"),
            contact_pipeline("cs_relax_even"),
            contact_pipeline("cs_relax_odd"),
            contact_pipeline("cs_relax_scatter"),
        ];
        // bound where a scene presses nothing: a posed vertex's worth of zeros
        let no_posed = buffer(device, "no posed vertices", 48, storage_usage);
        // (bound for the relax of a pressed mesh that has none, ADR-822)
        let no_ring = buffer(device, "no ring", 48, storage_usage);
        let no_ring_work = buffer(device, "no ring work", 48, storage_usage);
        MeshGpu {
            samples,
            contact_morph_layout,
            contact_layout,
            contact_depth_pipeline,
            contact_deform_pipeline,
            contact_relax_pipelines,
            no_posed,
            no_ring,
            no_ring_work,
            pipelines,
            hair_depth_pipelines,
            hair_front_pipelines,
            fibre_scene_layout,
            fibre_layers_pipelines,
            fibre_front_pipelines,
            fibre_full,
            shadow_pipeline,
            shadow_scene_layout,
            opacity_front_pipeline,
            opacity_pipeline,
            opacity_scene_layout,
            no_opacity_front,
            no_opacity,
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
            skin_table: skin_table_texture(device, queue),
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
                // trilinear through a smooth texture's mip chain (`mesh::mip_chain`), as the
                // CPU painter's `sample_lod`
                mipmap_filter: wgpu::MipmapFilterMode::Linear,
                ..Default::default()
            }),
            meshes: HashMap::new(),
            targets: Vec::new(),
            timer: super::scene_timer::SceneTimer::new(device, queue),
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
            vertices.extend(mesh.curvature[index].to_le_bytes());
            // its centre of rotation (ADR-814; its own place when it has none, unread then)
            vertices.extend(floats_bytes(
                mesh.centres.get(index).unwrap_or(&mesh.positions[index]),
            ));
            // its second UV set (ADR-817)
            vertices.extend(floats_bytes(&mesh.uvs2[index]));
            // how much of its draw's lag it takes (ADR-823; all when the mesh gives none)
            vertices.extend(
                mesh.loose_weight
                    .get(index)
                    .copied()
                    .unwrap_or(1.0)
                    .to_le_bytes(),
            );
            // how far short of a contact's surface it stops (ADR-824; none when the mesh gives none)
            vertices.extend(
                mesh.standoff
                    .get(index)
                    .copied()
                    .unwrap_or(0.0)
                    .to_le_bytes(),
            );
        }
        let vertex_buffer = buffer(
            device,
            "mesh vertices",
            vertices.len() as u64,
            // (read as storage too by the contact passes, ADR-805)
            wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::STORAGE,
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
        // its softness a vertex, for the ring round a contact (ADR-805)
        let softness = (!mesh.softness.is_empty()).then(|| {
            let bytes = floats_bytes(&mesh.softness);
            let softness = buffer(
                device,
                "mesh softness",
                bytes.len() as u64,
                wgpu::BufferUsages::STORAGE,
            );
            queue.write_buffer(&softness, 0, &bytes);
            softness
        });
        // (and, for a mesh that can be pressed, what its squeezed skin is relaxed by, ADR-822)
        let ring = if softness.is_some() {
            mesh_ring(device, queue, mesh)
        } else {
            None
        };
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
                turning: !mesh.centres.is_empty(),
                bounds: mesh.bounds.clone(),
                softness,
                ring,
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
        let color = texture("scene colour", COLOR_FORMAT, self.samples, attachment);
        // (readable: the fibres' passes take its nearest sample, ADR-813)
        let depth = texture(
            "scene depth",
            DEPTH_FORMAT,
            self.samples,
            attachment | wgpu::TextureUsages::TEXTURE_BINDING,
        );
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
            contacts: None,
            fibres: None,
        };
        if index < self.targets.len() {
            self.targets[index] = target;
        } else {
            self.targets.push(target);
        }
    }

    /// Keep scene `index`'s shadow maps as the two arrays `arrays` says (ADR-800), making
    /// them anew when either's size or layer count changed; none when the scene has no shadow
    /// this frame.
    fn shadow_target(
        &mut self,
        device: &wgpu::Device,
        index: usize,
        arrays: ShadowArrays,
        views: u32,
    ) {
        let target = &mut self.targets[index];
        if arrays.layers[0] == 0 || views == 0 {
            target.shadow = None;
            return;
        }
        if target
            .shadow
            .as_ref()
            .is_some_and(|shadow| shadow.arrays == arrays && shadow.views == views)
        {
            return;
        }
        // the hair's opacity maps (ADR-827): its nearest depth and its density, a layer a view
        let opacity_array = |label, format| {
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width: HAIR_OPACITY_SIDE,
                    height: HAIR_OPACITY_SIDE,
                    depth_or_array_layers: views,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            });
            let layers: Vec<_> = (0..views)
                .map(|layer| {
                    texture.create_view(&wgpu::TextureViewDescriptor {
                        label: Some(label),
                        dimension: Some(wgpu::TextureViewDimension::D2),
                        base_array_layer: layer,
                        array_layer_count: Some(1),
                        ..Default::default()
                    })
                })
                .collect();
            let whole = texture.create_view(&wgpu::TextureViewDescriptor {
                label: Some(label),
                dimension: Some(wgpu::TextureViewDimension::D2Array),
                ..Default::default()
            });
            (layers, whole)
        };
        let (opacity_front_layers, opacity_front) =
            opacity_array("scene hair nearest depth", DEPTH_FORMAT);
        let (opacity_layers, opacity) = opacity_array("scene hair opacity", HAIR_OPACITY_FORMAT);
        let mut layers: [Vec<wgpu::TextureView>; 2] = [Vec::new(), Vec::new()];
        let mut whole: [Option<wgpu::TextureView>; 2] = [None, None];
        for array in 0..2 {
            let (side, layer_count) = (arrays.sides[array], arrays.layers[array]);
            if layer_count == 0 || side == 0 {
                continue;
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
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            });
            layers[array] = (0..layer_count)
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
            whole[array] = Some(texture.create_view(&wgpu::TextureViewDescriptor {
                label: Some("scene shadow maps"),
                dimension: Some(wgpu::TextureViewDimension::D2Array),
                ..Default::default()
            }));
        }
        target.shadow = Some(ShadowTarget {
            arrays,
            layers,
            whole,
            views,
            opacity_front_layers,
            opacity_layers,
            opacity_front,
            opacity,
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
        // each scene's pressed vertices, all its `:deform` draws' (ADR-805): its posed buffer's size
        let mut pressed_totals: Vec<u32> = Vec::new();
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
            // where its casting hair can be (ADR-827: each view's `hair_rect`)
            let hair = scene
                .draws
                .iter()
                .filter(|draw| draw.material.fibres && draw.material.cast_shadow)
                .fold(None, |bounds, draw| {
                    let more = self
                        .meshes
                        .get(&draw.mesh)
                        .and_then(|mesh| draw_bounds(scene, draw, &mesh.bounds));
                    union(bounds, more)
                });
            scene_bytes.extend(scene_uniform(scene, &shadows, &emitter_plan(scene), hair));
            shadow_plans.push(shadows);
            let mut pressed_vertices = 0u32;
            // the scene's palettes, placed after the ones before
            let mut palette_starts = Vec::with_capacity(scene.palettes.len());
            for palette in &scene.palettes {
                palette_starts.push((palette_floats.len() / 16) as u32);
                // whole matrices only, so its joints' rotations below start where the shader
                // looks for them, a palette's joint count on (ADR-814)
                let whole = &palette[..palette.len() / 16 * 16];
                palette_floats.extend_from_slice(whole);
                palette_floats.extend(joint_rotation_floats(whole));
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
                let occlusion = material.occlusion.and_then(|handle| textures.get(&handle));
                // none for a hair surface, as the CPU painter reads none there
                let detail = material
                    .detail_texture()
                    .and_then(|handle| textures.get(&handle));
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
                    Surface::Hair => 4,
                    Surface::Strands => 5,
                    Surface::Eye => 6,
                };
                // and 512 for a `:fibres` strand (ADR-813): drawn at its real width, its coverage exact
                let bits = texture_bits(
                    [base, normal, occlusion, detail].map(|texture| {
                        texture.map(|texture| texture.filter != TextureFilter::Nearest)
                    }),
                    mesh.has_tangents,
                ) | if material.fibres { 512 } else { 0 }
                    // and 1024 for a skin's expression wrinkles (ADR-817): on a mesh with tangents,
                    // all four of its textures there, as the CPU's `wrinkle_textures`
                    | if mesh.has_tangents
                        && material.surface == Surface::Skin
                        && material.wrinkles.as_ref().is_some_and(|wrinkles| {
                            [wrinkles.compress, wrinkles.stretch, wrinkles.masks[0], wrinkles.masks[1]]
                                .iter()
                                .all(|handle| textures.contains_key(handle))
                        }) {
                        1024
                    } else {
                        0
                    };
                // and 2048 for a skin's regions map (ADR-826), when it is uploaded
                let bits = bits
                    | if material.surface == Surface::Skin
                        && material
                            .regions
                            .is_some_and(|handle| textures.contains_key(&handle))
                    {
                        2048
                    } else {
                        0
                    }
                    // (and 4096 when it was uploaded premultiplied, as a smooth texture is: its alpha
                    // is the oil, so its other channels come back multiplied by it unless undone)
                    | if material.surface == Surface::Skin
                        && material
                            .regions
                            .and_then(|handle| textures.get(&handle))
                            .is_some_and(|texture| texture.filter != TextureFilter::Nearest)
                    {
                        4096
                    } else {
                        0
                    };
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
                let (palette_start, palette_joints) = palette.unwrap_or((0, 0));
                // (2: its vertices turn about their centres, ADR-814)
                let skinned = match (mesh.skinned && palette_joints > 0, mesh.turning) {
                    (false, _) => 0u32,
                    (true, false) => 1,
                    (true, true) => 2,
                };
                // `DrawU::deform` (ADR-805): pressed by the scene's contacts, where its posed vertices
                // start in the scene's posed buffer, whether it has a softness, how many vertices
                // `DrawU::loose` (ADR-812): its flesh lagging its bones, where the lagging palette
                // starts and how many joints it holds (a skinned draw only, as the CPU poses it)
                let loose = crate::host::gui::mesh::loose_palette(scene, draw)
                    .filter(|_| skinned != 0)
                    .and_then(|(joints, amount)| {
                        let start = palette_starts.get(draw.loose?.palette)?;
                        Some([amount, *start as f32, (joints.len() / 16) as f32, 0.0])
                    });
                // posed once by the compute passes: pressed by contacts, or loose (with no contacts
                // the passes only pose it)
                let pressed = (!scene.contacts.is_empty() || loose.is_some())
                    && crate::host::gui::mesh::pressed(draw);
                let deform = if pressed {
                    let start = pressed_vertices;
                    pressed_vertices += mesh.vertex_count;
                    [
                        1,
                        start,
                        u32::from(mesh.softness.is_some()),
                        mesh.vertex_count,
                    ]
                } else {
                    [0; 4]
                };
                let mut bytes = draw_uniform(
                    draw,
                    [surface, bits, palette_start, skinned],
                    [
                        u32::from(morphed),
                        mesh.vertex_count,
                        palette_joints,
                        morph_start,
                    ],
                    deform,
                    loose.unwrap_or([0.0; 4]),
                );
                bytes.resize(DRAW_BYTES as usize, 0);
                let at = draw_bytes.len() / DRAW_BYTES as usize;
                draw_bytes.extend(bytes);
                plan.push(DrawPlan {
                    at,
                    mesh: draw.mesh,
                    double_sided: material.double_sided,
                    blended: material.surface == Surface::Blend,
                    casts: material.cast_shadow,
                    surface: material.surface,
                    pressed,
                    fibres: material.fibres,
                    hair_bounds: if material.fibres && material.cast_shadow {
                        draw_bounds(scene, draw, &mesh.bounds)
                    } else {
                        None
                    },
                });
            }
            plans.push(plan);
            pressed_totals.push(pressed_vertices);
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
            self.shadow_target(device, index, shadows.arrays, shadows.views.len() as u32);
            // its contact buffers (ADR-805): only while it presses any skin, so a scene without
            // contacts costs nothing for them
            let pressed = pressed_totals[index];
            let target = &mut self.targets[index];
            if pressed == 0 {
                target.contacts = None;
            } else {
                if target
                    .contacts
                    .as_ref()
                    .is_none_or(|contacts| contacts.capacity < pressed)
                {
                    let storage = wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST;
                    target.contacts = Some(ContactTarget {
                        posed: buffer(device, "posed vertices", u64::from(pressed) * 48, storage),
                        capacity: pressed,
                        contacts: buffer(
                            device,
                            "contacts",
                            16 + crate::host::gui::mesh::MAX_CONTACTS as u64 * 96,
                            wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                        ),
                        depths: buffer(
                            device,
                            "contact depths",
                            crate::host::gui::mesh::MAX_CONTACTS as u64 * 4,
                            storage,
                        ),
                    });
                }
                if let Some(contacts) = &target.contacts {
                    queue.write_buffer(&contacts.contacts, 0, &contact_words(scene));
                }
            }
            // its fibre buffers (ADR-813): only for a scene that draws `:fibres`, so a scene without
            // hair costs nothing for them
            let size = scene.pixel_size();
            let target = &mut self.targets[index];
            if plans[index].iter().any(|draw| draw.fibres) {
                if target
                    .fibres
                    .as_ref()
                    .is_none_or(|fibres| fibres.size != size)
                {
                    target.fibres = Some(super::fibres::FibreTarget::new(
                        device,
                        queue,
                        size,
                        self.samples,
                    ));
                }
            } else {
                target.fibres = None;
            }
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
        // the morph offsets, and a scene's posed vertices (ADR-805): made for each scene below
        let morph_group_for = |posed: &wgpu::Buffer| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("mesh morph offsets"),
                layout: &self.morph_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: self.morph_offset_buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: posed.as_entire_binding(),
                    },
                ],
            })
        };
        // the contact passes' (ADR-805): the morph offsets alone
        let contact_morph_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("contact morph offsets"),
            layout: &self.contact_morph_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: self.morph_offset_buffer.as_entire_binding(),
            }],
        });
        if let Some(timer) = &mut self.timer {
            timer.begin_frame(device);
        }
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
                    let occlusion = view(draw.material.occlusion).unwrap_or(&self.white);
                    // no tile: a flat, open texel (the normal map's stand-in is one)
                    let detail = view(draw.material.detail_texture()).unwrap_or(&self.flat);
                    // a skin's regions map (ADR-826): an open texel where none (read only with bit 2048)
                    let regions = view(draw.material.regions).unwrap_or(&self.white);
                    // its wrinkle maps and masks (ADR-817): flat maps and open masks where none
                    let wrinkles = draw.material.wrinkles.as_ref();
                    let compress = view(wrinkles.map(|w| w.compress)).unwrap_or(&self.flat);
                    let stretch = view(wrinkles.map(|w| w.stretch)).unwrap_or(&self.flat);
                    let first_mask = view(wrinkles.map(|w| w.masks[0])).unwrap_or(&self.white);
                    let second_mask = view(wrinkles.map(|w| w.masks[1])).unwrap_or(&self.white);
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
                            wgpu::BindGroupEntry {
                                binding: 3,
                                resource: wgpu::BindingResource::TextureView(occlusion),
                            },
                            wgpu::BindGroupEntry {
                                binding: 4,
                                resource: wgpu::BindingResource::TextureView(detail),
                            },
                            wgpu::BindGroupEntry {
                                binding: 5,
                                resource: wgpu::BindingResource::TextureView(regions),
                            },
                            wgpu::BindGroupEntry {
                                binding: 13,
                                resource: wgpu::BindingResource::TextureView(compress),
                            },
                            wgpu::BindGroupEntry {
                                binding: 14,
                                resource: wgpu::BindingResource::TextureView(stretch),
                            },
                            wgpu::BindGroupEntry {
                                binding: 15,
                                resource: wgpu::BindingResource::TextureView(first_mask),
                            },
                            wgpu::BindGroupEntry {
                                binding: 16,
                                resource: wgpu::BindingResource::TextureView(second_mask),
                            },
                        ],
                    })
                })
                .collect();
            let target = &self.targets[index];
            let scene_offset = (index as u64 * SCENE_BYTES) as u32;
            let morph_group = morph_group_for(
                target
                    .contacts
                    .as_ref()
                    .map_or(&self.no_posed, |contacts| &contacts.posed),
            );
            // its skin pressed by its contacts (ADR-805), before anything draws it: each pressed
            // vertex posed once, each contact's deepest press found over them all; then pressed
            if let Some(contacts) = &target.contacts {
                encoder.clear_buffer(&contacts.depths, 0, None);
                fn ring_entry<'a>(
                    binding: u32,
                    buffer: Option<&'a wgpu::Buffer>,
                    stand_in: &'a wgpu::Buffer,
                ) -> wgpu::BindGroupEntry<'a> {
                    wgpu::BindGroupEntry {
                        binding,
                        resource: buffer.unwrap_or(stand_in).as_entire_binding(),
                    }
                }
                let pressed: Vec<_> = plans[index]
                    .iter()
                    .filter(|draw| draw.pressed)
                    .filter_map(|draw| {
                        let mesh = self.meshes.get(&draw.mesh)?;
                        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                            label: Some("contacts"),
                            layout: &self.contact_layout,
                            entries: &[
                                wgpu::BindGroupEntry {
                                    binding: 8,
                                    resource: mesh.vertices.as_entire_binding(),
                                },
                                wgpu::BindGroupEntry {
                                    binding: 9,
                                    resource: contacts.contacts.as_entire_binding(),
                                },
                                wgpu::BindGroupEntry {
                                    binding: 10,
                                    resource: contacts.depths.as_entire_binding(),
                                },
                                wgpu::BindGroupEntry {
                                    binding: 11,
                                    resource: contacts.posed.as_entire_binding(),
                                },
                                wgpu::BindGroupEntry {
                                    binding: 12,
                                    resource: mesh
                                        .softness
                                        .as_ref()
                                        .unwrap_or(&self.no_posed)
                                        .as_entire_binding(),
                                },
                                // (its squeezed skin relaxed, ADR-822; stand-ins without it)
                                ring_entry(
                                    17,
                                    mesh.ring.as_ref().map(|ring| &ring.index),
                                    &self.no_ring,
                                ),
                                wgpu::BindGroupEntry {
                                    binding: 18,
                                    resource: mesh
                                        .ring
                                        .as_ref()
                                        .map_or(&self.no_ring_work, |ring| &ring.data)
                                        .as_entire_binding(),
                                },
                            ],
                        });
                        let points = mesh.ring.as_ref().map_or(0, |ring| ring.points);
                        Some((draw.at, mesh.vertex_count, points, group))
                    })
                    .collect();
                let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                    label: Some("scene contacts"),
                    timestamp_writes: None,
                });
                // (posed; then its squeezed skin relaxed, ADR-822: gathered by point, masked by its
                // squeeze, relaxed `RELAX_ITERATIONS` times between two halves, scattered back to
                // its vertices; then pressed)
                let [gather, mask, even, odd, scatter] = &self.contact_relax_pipelines;
                let mut steps: Vec<(&wgpu::ComputePipeline, bool)> = vec![
                    (&self.contact_depth_pipeline, false),
                    (gather, true),
                    (mask, true),
                ];
                for iteration in 0..RELAX_ITERATIONS {
                    steps.push((if iteration % 2 == 0 { even } else { odd }, true));
                }
                steps.push((scatter, false));
                steps.push((&self.contact_deform_pipeline, false));
                let last = steps.len() - 1;
                for (step, (pipeline, by_point)) in steps.iter().enumerate() {
                    let relaxing = step > 0 && step < last;
                    pass.set_pipeline(pipeline);
                    pass.set_bind_group(0, &shadow_scene_group, &[scene_offset]);
                    pass.set_bind_group(2, &contact_morph_group, &[]);
                    for (at, vertex_count, points, group) in &pressed {
                        if relaxing && *points == 0 {
                            continue;
                        }
                        pass.set_bind_group(1, &draw_group, &[(*at as u64 * DRAW_BYTES) as u32]);
                        pass.set_bind_group(3, group, &[]);
                        let count = if *by_point { *points } else { *vertex_count };
                        pass.dispatch_workgroups(count.div_ceil(64), 1, 1);
                    }
                }
            }
            // the shadow maps: every casting draw, once a view, the view by instance index, into
            // the layer of the array its `map` names (ADR-800)
            if let Some(shadow) = &target.shadow {
                let drawn: Vec<_> = shadow_plans[index]
                    .views
                    .iter()
                    .enumerate()
                    .filter_map(|(view_index, view)| {
                        shadow
                            .layers
                            .get(view.map.array as usize)
                            .and_then(|layers| layers.get(view.map.layer as usize))
                            .map(|layer| (view_index, view, layer))
                    })
                    .collect();
                // the span of the scene's shadow passes: the first one's start, the last one's
                // end (ADR-801)
                let stamps = match &mut self.timer {
                    Some(timer) if !drawn.is_empty() => timer.shadow_stamps(index),
                    _ => None,
                };
                // (the span ends on the last hair opacity pass, below: they are shadow passes too)
                for (order, (view_index, view, layer)) in drawn.into_iter().enumerate() {
                    let timestamp_writes = stamps.and_then(|(start, _)| {
                        self.timer
                            .as_ref()?
                            .writes((order == 0).then_some(start), None)
                    });
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
                        timestamp_writes,
                        occlusion_query_set: None,
                        multiview_mask: None,
                    });
                    let side = view.resolution as f32;
                    pass.set_viewport(0.0, 0.0, side, side, 0.0, 1.0);
                    pass.set_pipeline(&self.shadow_pipeline);
                    pass.set_bind_group(0, &shadow_scene_group, &[scene_offset]);
                    let instance = view_index as u32;
                    // (what is opaque: a `:fibres` mesh's shadow is its opacity maps', below)
                    for (draw, material) in plans[index].iter().zip(&materials) {
                        if !draw.casts || draw.fibres {
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
                // the hair's opacity maps (ADR-827), a layer each view: its nearest depth, then
                // its density in the slices behind it; cleared where no hair casts, so a scene
                // without hair reads none
                let opacity_scene_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("hair opacity scene"),
                    layout: &self.opacity_scene_layout,
                    entries: &[
                        scene_uniform_entry(),
                        palette_entry(),
                        wgpu::BindGroupEntry {
                            binding: 7,
                            resource: wgpu::BindingResource::TextureView(&shadow.opacity_front),
                        },
                    ],
                });
                let hair: Vec<_> = plans[index]
                    .iter()
                    .zip(&materials)
                    .filter(|(draw, _)| draw.casts && draw.fibres)
                    .collect();
                let side = HAIR_OPACITY_SIDE as f32;
                let last_view = shadow_plans[index].views.len().saturating_sub(1);
                for (view_index, view) in shadow_plans[index].views.iter().enumerate() {
                    let instance = view_index as u32;
                    // (only the hair this view can see: a box beyond its frustum is not drawn;
                    // one with no bounds known always is)
                    let in_view: Vec<_> = hair
                        .iter()
                        .filter(|(draw, _)| {
                            draw.hair_bounds
                                .is_none_or(|bounds| seen_by(&view.view_proj, bounds, 0.05))
                        })
                        .collect();
                    let draw_hair = |pass: &mut wgpu::RenderPass| {
                        for (draw, material) in &in_view {
                            let mesh = &self.meshes[&draw.mesh];
                            pass.set_bind_group(
                                1,
                                &draw_group,
                                &[(draw.at as u64 * DRAW_BYTES) as u32],
                            );
                            pass.set_bind_group(2, &morph_group, &[]);
                            pass.set_bind_group(3, *material, &[]);
                            pass.set_vertex_buffer(0, mesh.vertices.slice(..));
                            pass.set_index_buffer(
                                mesh.indices.slice(..),
                                wgpu::IndexFormat::Uint32,
                            );
                            // (the first share of its strands, ordered at random, each drawn as
                            // wide as the strands it stands for)
                            let drawn = (mesh.index_count / FIBRE_SHADOW_SHARE).max(3) / 3 * 3;
                            pass.draw_indexed(0..drawn, 0, instance..instance + 1);
                        }
                    };
                    {
                        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                            label: Some("hair nearest depth"),
                            color_attachments: &[],
                            depth_stencil_attachment: Some(
                                wgpu::RenderPassDepthStencilAttachment {
                                    view: &shadow.opacity_front_layers[view_index],
                                    depth_ops: Some(wgpu::Operations {
                                        load: wgpu::LoadOp::Clear(1.0),
                                        store: wgpu::StoreOp::Store,
                                    }),
                                    stencil_ops: None,
                                },
                            ),
                            timestamp_writes: None,
                            occlusion_query_set: None,
                            multiview_mask: None,
                        });
                        if !in_view.is_empty() {
                            pass.set_viewport(0.0, 0.0, side, side, 0.0, 1.0);
                            pass.set_pipeline(&self.opacity_front_pipeline);
                            pass.set_bind_group(0, &shadow_scene_group, &[scene_offset]);
                            draw_hair(&mut pass);
                        }
                    }
                    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("hair opacity"),
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view: &shadow.opacity_layers[view_index],
                            depth_slice: None,
                            resolve_target: None,
                            ops: wgpu::Operations {
                                load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                                store: wgpu::StoreOp::Store,
                            },
                        })],
                        depth_stencil_attachment: None,
                        // (the shadow passes' span ends here, ADR-801)
                        timestamp_writes: stamps.and_then(|(_, end)| {
                            self.timer
                                .as_ref()?
                                .writes(None, (view_index == last_view).then_some(end))
                        }),
                        occlusion_query_set: None,
                        multiview_mask: None,
                    });
                    if !in_view.is_empty() {
                        pass.set_viewport(0.0, 0.0, side, side, 0.0, 1.0);
                        pass.set_pipeline(&self.opacity_pipeline);
                        pass.set_bind_group(0, &opacity_scene_group, &[scene_offset]);
                        draw_hair(&mut pass);
                    }
                }
            }
            // array 0 at binding 2, array 1 at binding 5; the cleared layer for one not kept
            let shadow_array = |array: usize| {
                target
                    .shadow
                    .as_ref()
                    .and_then(|shadow| shadow.whole[array].as_ref())
                    .unwrap_or(&self.no_shadows)
            };
            let (shadow_maps, smaller_shadow_maps) = (shadow_array(0), shadow_array(1));
            // the hair's opacity maps (ADR-827), or the empty ones
            let (hair_nearest_maps, hair_opacity_maps) = match &target.shadow {
                Some(shadow) => (&shadow.opacity_front, &shadow.opacity),
                None => (&self.no_opacity_front, &self.no_opacity),
            };
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
                    wgpu::BindGroupEntry {
                        binding: 4,
                        resource: wgpu::BindingResource::TextureView(&self.skin_table),
                    },
                    wgpu::BindGroupEntry {
                        binding: 5,
                        resource: wgpu::BindingResource::TextureView(smaller_shadow_maps),
                    },
                    wgpu::BindGroupEntry {
                        binding: 7,
                        resource: wgpu::BindingResource::TextureView(hair_nearest_maps),
                    },
                    wgpu::BindGroupEntry {
                        binding: 8,
                        resource: wgpu::BindingResource::TextureView(hair_opacity_maps),
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
            let colour_stamps = self
                .timer
                .as_mut()
                .and_then(|timer| timer.colour_stamps(index));
            let timestamp_writes = colour_stamps
                .and_then(|(start, end)| self.timer.as_ref()?.writes(Some(start), Some(end)));
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
                        // kept for the fibres' passes when the scene has them (ADR-813)
                        store: if target.fibres.is_some() {
                            wgpu::StoreOp::Store
                        } else {
                            wgpu::StoreOp::Discard
                        },
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_bind_group(0, &scene_group, &[scene_offset]);
            let is_hair =
                |draw: &DrawPlan| matches!(draw.surface, Surface::Hair | Surface::Strands);
            let prepass = hair_prepass();
            // the hair's coverage to depth first (ADR-802), so its colour is shaded only where
            // it is the hair in front: every layer of wisps behind is rejected before shading
            if prepass {
                for (draw, material) in plans[index].iter().zip(&materials) {
                    if !is_hair(draw) || draw.fibres {
                        continue;
                    }
                    let mesh = &self.meshes[&draw.mesh];
                    pass.set_pipeline(&self.hair_depth_pipelines[draw.double_sided as usize]);
                    pass.set_bind_group(1, &draw_group, &[(draw.at as u64 * DRAW_BYTES) as u32]);
                    pass.set_bind_group(2, &morph_group, &[]);
                    pass.set_bind_group(3, material, &[]);
                    pass.set_vertex_buffer(0, mesh.vertices.slice(..));
                    pass.set_index_buffer(mesh.indices.slice(..), wgpu::IndexFormat::Uint32);
                    pass.draw_indexed(0..mesh.index_count, 0, 0..1);
                }
            }
            for (draw, material) in plans[index].iter().zip(&materials) {
                // (a `:fibres` draw is drawn by the fibre passes after this one, ADR-813)
                if draw.fibres {
                    continue;
                }
                let mesh = &self.meshes[&draw.mesh];
                // each draw's own time on the card, under BROOD_GUI_GPU_TIMES=draws (ADR-801)
                let stamp = self.timer.as_mut().and_then(|timer| {
                    timer.draw_stamp(index, draw.mesh, draw.surface, mesh.index_count / 3)
                });
                if let (Some(query), Some(timer)) = (stamp, &self.timer) {
                    pass.write_timestamp(timer.query_set(), query);
                }
                if prepass && is_hair(draw) {
                    pass.set_pipeline(&self.hair_front_pipelines[draw.double_sided as usize]);
                } else {
                    pass.set_pipeline(
                        &self.pipelines[draw.double_sided as usize * 2 + draw.blended as usize],
                    );
                }
                pass.set_bind_group(1, &draw_group, &[(draw.at as u64 * DRAW_BYTES) as u32]);
                pass.set_bind_group(2, &morph_group, &[]);
                pass.set_bind_group(3, material, &[]);
                pass.set_vertex_buffer(0, mesh.vertices.slice(..));
                pass.set_index_buffer(mesh.indices.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..mesh.index_count, 0, 0..1);
            }
            let end = self
                .timer
                .as_mut()
                .and_then(|timer| timer.draws_end_stamp(index));
            if let (Some(query), Some(timer)) = (end, &self.timer) {
                pass.write_timestamp(timer.query_set(), query);
            }
            drop(pass);
            // the scene's `:fibres` (ADR-813: `fibres.rs`): after its colour pass, over it
            let Some(fibre_target) = &target.fibres else {
                continue;
            };
            let full = &self.fibre_full;
            let full_group = fibre_target.group(device, full, &target.depth);
            let fibre_scene_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("fibre scene"),
                layout: &self.fibre_scene_layout,
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
                    wgpu::BindGroupEntry {
                        binding: 4,
                        resource: wgpu::BindingResource::TextureView(&self.skin_table),
                    },
                    wgpu::BindGroupEntry {
                        binding: 5,
                        resource: wgpu::BindingResource::TextureView(smaller_shadow_maps),
                    },
                    wgpu::BindGroupEntry {
                        binding: 7,
                        resource: wgpu::BindingResource::TextureView(hair_nearest_maps),
                    },
                    wgpu::BindGroupEntry {
                        binding: 8,
                        resource: wgpu::BindingResource::TextureView(hair_opacity_maps),
                    },
                    wgpu::BindGroupEntry {
                        binding: 6,
                        resource: fibre_target.layers.as_entire_binding(),
                    },
                ],
            });
            encoder.clear_buffer(&fibre_target.layers, 0, None);
            let depth_attachment = |load| {
                Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &fibre_target.depth,
                    depth_ops: Some(wgpu::Operations {
                        load,
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                })
            };
            fn colour_attachment(
                view: &wgpu::TextureView,
                load: wgpu::LoadOp<wgpu::Color>,
            ) -> Option<wgpu::RenderPassColorAttachment<'_>> {
                Some(wgpu::RenderPassColorAttachment {
                    view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load,
                        store: wgpu::StoreOp::Store,
                    },
                })
            }
            let fibre_draws: Vec<_> = plans[index]
                .iter()
                .zip(&materials)
                .filter(|(draw, _)| draw.fibres)
                .collect();
            // 1. the nearest of the scene's depth samples, the fibres' own depth
            {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("fibre depth copy"),
                    color_attachments: &[],
                    depth_stencil_attachment: depth_attachment(wgpu::LoadOp::Clear(1.0)),
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                });
                pass.set_pipeline(&full.depth_copy);
                pass.set_bind_group(0, &full_group, &[]);
                pass.draw(0..3, 0..1);
            }
            // 2. each fibre fragment's depth among its pixel's nearest three, its coverage out
            //    of the pixel's transmittance; then 4. the nearest three shaded: one geometry pass
            //    each, the same draws
            let geometry = |encoder: &mut wgpu::CommandEncoder,
                            label: &str,
                            view: &wgpu::TextureView,
                            clear: wgpu::Color,
                            pipelines: &[wgpu::RenderPipeline; 2]| {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some(label),
                    color_attachments: &[colour_attachment(view, wgpu::LoadOp::Clear(clear))],
                    depth_stencil_attachment: depth_attachment(wgpu::LoadOp::Load),
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                });
                pass.set_bind_group(0, &fibre_scene_group, &[scene_offset]);
                for (draw, material) in &fibre_draws {
                    let mesh = &self.meshes[&draw.mesh];
                    pass.set_pipeline(&pipelines[draw.double_sided as usize]);
                    pass.set_bind_group(1, &draw_group, &[(draw.at as u64 * DRAW_BYTES) as u32]);
                    pass.set_bind_group(2, &morph_group, &[]);
                    pass.set_bind_group(3, *material, &[]);
                    pass.set_vertex_buffer(0, mesh.vertices.slice(..));
                    pass.set_index_buffer(mesh.indices.slice(..), wgpu::IndexFormat::Uint32);
                    pass.draw_indexed(0..mesh.index_count, 0, 0..1);
                }
            };
            geometry(
                encoder,
                "fibre layers",
                &fibre_target.transmit,
                wgpu::Color::WHITE,
                &self.fibre_layers_pipelines,
            );
            // 3. the third nearest depth, where a pixel has three
            {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("fibre third depth"),
                    color_attachments: &[],
                    depth_stencil_attachment: depth_attachment(wgpu::LoadOp::Load),
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                });
                pass.set_pipeline(&full.kth);
                pass.set_bind_group(0, &full_group, &[]);
                pass.draw(0..3, 0..1);
            }
            geometry(
                encoder,
                "fibre front",
                &fibre_target.accum,
                wgpu::Color::TRANSPARENT,
                &self.fibre_front_pipelines,
            );
            // 5. the fibres over the scene's picture
            {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("fibre resolve"),
                    color_attachments: &[colour_attachment(&target.resolve, wgpu::LoadOp::Load)],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                });
                pass.set_pipeline(&full.resolve);
                pass.set_bind_group(0, &full_group, &[]);
                pass.draw(0..3, 0..1);
            }
        }
        if let Some(timer) = &mut self.timer {
            timer.end_frame(encoder);
        }
    }
}

/// A draw's texture bits (`DrawU::flags.y`): for its base, normal, occlusion and detail
/// texture in turn — `None` when the draw has none, else whether it was uploaded premultiplied
/// (a smooth texture is) — a bit that it is there and a bit that it is premultiplied (1 and 2,
/// 4 and 8, 16 and 32, 64 and 128); then 256 when the detail tile's normal tilts the
/// surface's, which takes the mesh's tangents (ADR-797; its cavity does not).
pub(super) fn texture_bits(textures: [Option<bool>; 4], has_tangents: bool) -> u32 {
    let mut bits = 0;
    for (slot, texture) in textures.iter().enumerate() {
        bits |= match texture {
            Some(premultiplied) => (1 | (u32::from(*premultiplied) << 1)) << (2 * slot),
            None => 0,
        };
    }
    if textures[3].is_some() && has_tangents {
        bits |= 256;
    }
    bits
}

/// `draw`'s `DrawU` uniform (the WGSL struct in `MESH_SRC`) before its padding to
/// [`DRAW_BYTES`]: its model matrix and material, then `flags` (surface, texture bits, palette
/// offset, skinned) and `morph_info` (morphed, vertex count, palette joint count, first offset).
pub(super) fn draw_uniform(
    draw: &crate::host::gui::mesh::Draw,
    flags: [u32; 4],
    morph_info: [u32; 4],
    deform: [u32; 4],
    loose: [f32; 4],
) -> Vec<u8> {
    let material = &draw.material;
    let mut floats = Vec::with_capacity(32);
    floats.extend_from_slice(&draw.model);
    floats.extend_from_slice(&material.color);
    // `DrawU::skin`: what turns the mesh's curvature into the skin table's, then the `:coat`'s
    // strength and roughness (ADR-795; 0: none)
    let [coat_strength, coat_roughness] = material.coat.unwrap_or([0.0, 0.0]);
    floats.extend_from_slice(&[material.scatter_scale(), coat_strength, coat_roughness, 0.0]);
    // (the third: a `:fibres` surface's `:density`, how much of its baked hair count dims its light)
    floats.extend_from_slice(&[
        material.roughness,
        material.specular,
        material.fibre_density,
        material.cutoff,
    ]);
    let lobes = material.lobes();
    floats.extend_from_slice(&[lobes[0], lobes[1], lobes[2], material.f0()]);
    let mut bytes = floats_bytes(&floats);
    bytes.extend(words_bytes(&flags));
    bytes.extend(words_bytes(&morph_info));
    // a `:strands` surface's width at the root and the tip, and a hair surface's two
    // highlights' shifts (`DrawU::strand`, ADR-799)
    bytes.extend(floats_bytes(&[
        material.strand_width[0],
        material.strand_width[1],
        material.hair_shift[0],
        material.hair_shift[1],
    ]));
    // `DrawU::detail`: the detail tile's repeats, its normal's strength, its cavity's amount
    bytes.extend(floats_bytes(&[
        material.detail_scale[0],
        material.detail_scale[1],
        material.detail_strength,
        material.detail_cavity,
    ]));
    bytes.extend(floats_bytes(&eye_floats(material)));
    // `DrawU::sheen`: a hair surface's second highlight's colour, the fourth float 1 when given
    let sheen = material.sheen.map_or([0.0; 4], |[r, g, b]| [r, g, b, 1.0]);
    bytes.extend(floats_bytes(&sheen));
    // `DrawU::deform`: pressed by the scene's contacts, its first posed vertex, whether it has a
    // softness, its vertex count (ADR-805)
    bytes.extend(words_bytes(&deform));
    // `DrawU::loose`: its flesh lagging its bones, the amount (0 none), its lagging palette's
    // offset and joint count (ADR-812)
    bytes.extend(floats_bytes(&loose));
    // `DrawU::clamp_info` and `clamps`: the planes its strands keep to (ADR-813)
    let planes = &material.clamp_planes[..material
        .clamp_planes
        .len()
        .min(crate::host::gui::mesh::MAX_CLAMP_PLANES)];
    // (and the UV set its `:wrinkles` read by, ADR-817)
    let wrinkle_uv = material
        .wrinkles
        .as_ref()
        .map_or(0.0, |wrinkles| wrinkles.uv as f32);
    // (and for a `:fibres` draw, how many of its strands the shadow pass stands each one it draws
    // for: `FIBRE_SHADOW_SHARE`, ADR-813)
    let shadow_share = if material.fibres {
        FIBRE_SHADOW_SHARE as f32
    } else {
        1.0
    };
    // (and the share of the light a `:fibres` strand blocks: its shadow fragments kept so often)
    let shadow_keep = if material.fibres {
        material.fibre_shadow_keep
    } else {
        1.0
    };
    bytes.extend(floats_bytes(&[
        planes.len() as f32,
        wrinkle_uv,
        shadow_share,
        shadow_keep,
    ]));
    let mut clamps = [0.0f32; crate::host::gui::mesh::MAX_CLAMP_PLANES * 4];
    for (slot, plane) in planes.iter().enumerate() {
        clamps[slot * 4..slot * 4 + 4].copy_from_slice(plane);
    }
    bytes.extend(floats_bytes(&clamps));
    // `DrawU::wrinkles`: its eight wrinkle zones' weights (ADR-817), zero without
    bytes.extend(floats_bytes(
        &material
            .wrinkles
            .as_ref()
            .map_or([0.0; 8], |wrinkles| wrinkles.weights),
    ));
    // `DrawU::fibre_look` and `fibre_cut`: a `:fibres` surface's look (ADR-825)
    bytes.extend(floats_bytes(&material.fibre_look));
    bytes.extend(floats_bytes(&material.fibre_cut));
    bytes
}

/// A material's `:eye` as `DrawU` holds it (ADR-796), all zeros for any surface but `:eye`:
/// `eye_centres` (each eye's centre at rest; the first's fourth float how many eyes there
/// are), `eye_axes`, `eye_up` (and the refractive index), `eye_shape` (the eyeball's, iris's
/// and cornea's radius, the iris's depth), `eye_uv` (each eye's iris in the texture) and
/// `eye_iris` (the iris's radius in the texture, the pupil's share of it, the bowl's slope).
fn eye_floats(material: &crate::host::gui::mesh::Material) -> [f32; 32] {
    let mut floats = [0.0f32; 32];
    let Some(eye) = material.eye_optics() else {
        return floats;
    };
    for (slot, at) in eye.eyes.iter().take(MAX_EYES).enumerate() {
        floats[slot * 4..slot * 4 + 3].copy_from_slice(&at.centre);
        floats[8 + slot * 4..8 + slot * 4 + 3].copy_from_slice(&at.axis);
        floats[24 + slot * 2..24 + slot * 2 + 2].copy_from_slice(&at.iris_uv);
    }
    floats[3] = eye.eyes.len().min(MAX_EYES) as f32;
    floats[16..19].copy_from_slice(&eye.up);
    floats[19] = eye.ior;
    floats[20..24].copy_from_slice(&[
        eye.radius,
        eye.iris_radius,
        eye.cornea_radius,
        eye.iris_depth,
    ]);
    floats[28] = eye.iris_uv_radius;
    floats[29] = eye.pupil;
    floats[30] = eye.concavity;
    // the limbal ring (`eye_iris.w`, `eye_axes[1].w`: two spare floats)
    floats[31] = eye.limbus;
    floats[15] = eye.limbus_width;
    floats
}

/// One draw of a scene as the passes issue it.
struct DrawPlan {
    /// Its `DrawU`'s slot in the frame's draw buffer.
    at: usize,
    mesh: u32,
    double_sided: bool,
    blended: bool,
    casts: bool,
    /// How it is shaded: what a draw's GPU time is told by (ADR-801).
    surface: Surface,
    /// Pressed by the scene's contacts (ADR-805): its vertices posed and deformed by the compute
    /// passes, read from the scene's posed buffer.
    pressed: bool,
    /// A `:fibres` draw (ADR-813): drawn by the fibre passes after the colour pass, not in it.
    fibres: bool,
    /// A casting `:fibres` draw's world bounds this frame (ADR-827): the shadow views it is in
    /// (`seen_by`) are the only ones it is drawn into for its opacity maps.
    hair_bounds: Option<Bounds>,
}

/// Whether a world box (grown by `margin` each way, for strands drawn wider than their centres)
/// may be in the view `view_proj` looks through: not when all eight corners lie beyond one of its
/// clip planes (ADR-827: a cube light's faces mostly never see her hair).
fn seen_by(view_proj: &[f32; 16], bounds: Bounds, margin: f32) -> bool {
    let corners: Vec<[f32; 4]> = box_corners(bounds, margin)
        .into_iter()
        .map(|corner| transform(view_proj, corner, 1.0))
        .collect();
    let beyond = |plane: fn(&[f32; 4]) -> bool| corners.iter().all(plane);
    !(beyond(|clip| clip[0] < -clip[3])
        || beyond(|clip| clip[0] > clip[3])
        || beyond(|clip| clip[1] < -clip[3])
        || beyond(|clip| clip[1] > clip[3])
        || beyond(|clip| clip[2] < 0.0)
        || beyond(|clip| clip[2] > clip[3]))
}

/// The eight corners of a world box grown by `margin` each way.
fn box_corners(bounds: Bounds, margin: f32) -> [[f32; 3]; 8] {
    std::array::from_fn(|corner| {
        std::array::from_fn(|axis| {
            if corner >> axis & 1 == 0 {
                bounds[0][axis] - margin
            } else {
                bounds[1][axis] + margin
            }
        })
    })
}

/// Where a scene's casting hair can be in a shadow view's opacity maps (ADR-827): its world box
/// projected, in their texels (left, top, right, bottom), grown by the widest penumbra search
/// (16 texels, and one for the bilinear read); `[1, 1, 0, 0]`, nothing, without hair in the view.
/// A receiver outside it reads no hair without a texel loaded.
fn hair_rect(view_proj: &[f32; 16], hair: Option<Bounds>) -> [f32; 4] {
    let nothing = [1.0, 1.0, 0.0, 0.0];
    let side = HAIR_OPACITY_SIDE as f32;
    let Some(bounds) = hair else {
        return nothing;
    };
    if !seen_by(view_proj, bounds, 0.05) {
        return nothing;
    }
    let mut rect = [f32::MAX, f32::MAX, f32::MIN, f32::MIN];
    for corner in box_corners(bounds, 0.05) {
        let clip = transform(view_proj, corner, 1.0);
        // (a corner behind a perspective view's eye: the whole map)
        if clip[3] <= 1e-6 {
            return [0.0, 0.0, side, side];
        }
        let x = (clip[0] / clip[3] * 0.5 + 0.5) * side;
        let y = (0.5 - clip[1] / clip[3] * 0.5) * side;
        rect = [
            rect[0].min(x),
            rect[1].min(y),
            rect[2].max(x),
            rect[3].max(y),
        ];
    }
    let grow = 17.0;
    [
        (rect[0] - grow).max(0.0),
        (rect[1] - grow).max(0.0),
        (rect[2] + grow).min(side),
        (rect[3] + grow).min(side),
    ]
}

/// `scene`'s `SceneU` uniform (the WGSL struct in `MESH_SRC`), padded to [`SCENE_BYTES`].
fn scene_uniform(
    scene: &Scene,
    shadows: &ShadowPlan,
    emitters: &EmitterPlan,
    hair: Option<Bounds>,
) -> Vec<u8> {
    let mut bytes = words_bytes(&scene_words(scene, shadows, emitters, hair));
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
        ambient_sh: None,
        exposure: 1.0,
        ambient_occlusion: None,
        emitters: Vec::new(),
        background: [0.0; 4],
        palettes: Vec::new(),
        contacts: Vec::new(),
        contact_drags: Vec::new(),
        contact_skips: Vec::new(),
        contact_onlys: Vec::new(),
        contact_withins: Vec::new(),
        draws: Vec::new(),
    };
    scene_words(
        &scene,
        &shadow_plan(&scene, None),
        &emitter_plan(&scene),
        None,
    )
    .len()
        * 4
}

/// The words of `scene`'s `SceneU`: its camera, its lights, its ambient and exposure, and the
/// shadow views `shadows` plans.
fn scene_words(
    scene: &Scene,
    shadows: &ShadowPlan,
    emitters: &EmitterPlan,
    hair: Option<Bounds>,
) -> Vec<u32> {
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
                // a soft shadow's light size, and its softness in metres (the shader's
                // `shadow_softness` turns it into texels where the receiver stands, as
                // `mesh::shadow_softness` does; 0 and no size: the 3×3 filter)
                floats(
                    &mut words,
                    &[direction[0], direction[1], direction[2], light.size],
                );
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
                // shaped: glossy surfaces show its shape, not a Blinn-Phong highlight (ADR-795)
                words.extend_from_slice(&[kind, first, count, u32::from(emitters.shaped[index])]);
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
            // spare since ADR-800 (it was the one layer side): each view's `map` has its own
            0.0,
            f32::from(u8::from(scene.ambient_sh.is_some())),
        ],
    );
    for index in 0..9 {
        let coefficient = scene
            .ambient_sh
            .map_or([0.0; 3], |coefficients| coefficients[index]);
        floats(
            &mut words,
            &[coefficient[0], coefficient[1], coefficient[2], 0.0],
        );
    }
    for index in 0..MAX_SHADOW_VIEWS {
        match shadows.views.get(index) {
            Some(view) => {
                floats(&mut words, &view.view_proj);
                floats(
                    &mut words,
                    &[
                        view.texel,
                        view.resolution as f32,
                        view.depth_range[0],
                        view.depth_range[1],
                    ],
                );
                // where its map is kept (ADR-800): its array's side, the array, its layer; and
                // whether it is a perspective view (1), for the hair's opacity maps (ADR-827)
                floats(
                    &mut words,
                    &[
                        view.map.side.max(1) as f32,
                        view.map.array as f32,
                        view.map.layer as f32,
                        if view.perspective { 1.0 } else { 0.0 },
                    ],
                );
                // where its hair can be in its opacity maps (ADR-827)
                floats(&mut words, &hair_rect(&view.view_proj, hair));
            }
            None => words.extend_from_slice(&[0; 28]),
        }
    }
    // the ambient occlusion: its radius (0: none), strength, and view (one past its index, 0:
    // none)
    let (radius, strength) = match (scene.ambient_occlusion, shadows.occlusion) {
        (Some(settings), Some(_)) => (settings.radius, settings.strength),
        _ => (0.0, 0.0),
    };
    floats(
        &mut words,
        &[
            radius,
            strength,
            shadows.occlusion.map_or(0.0, |view| (view + 1) as f32),
            0.0,
        ],
    );
    // the picture's size in pixels, for a strand's pixel width (`mesh::strand_vertex`)
    // … and how many emitters follow
    let (width, height) = scene.pixel_size();
    floats(
        &mut words,
        &[
            width as f32,
            height as f32,
            emitters.emitters.len().min(MAX_EMITTERS) as f32,
            0.0,
        ],
    );
    // the shapes glossy surfaces show (`mesh::PlannedEmitter`, the shader's `Emitter`)
    for index in 0..MAX_EMITTERS {
        match emitters.emitters.get(index) {
            Some(emitter) => {
                let [centre, facing, right, radiance] = [
                    emitter.centre,
                    emitter.facing,
                    emitter.right,
                    emitter.radiance,
                ];
                floats(
                    &mut words,
                    &[centre[0], centre[1], centre[2], emitter.shape.code() as f32],
                );
                floats(
                    &mut words,
                    &[
                        facing[0],
                        facing[1],
                        facing[2],
                        emitter.light.map_or(0.0, |light| (light + 1) as f32),
                    ],
                );
                floats(&mut words, &[right[0], right[1], right[2], emitter.half[0]]);
                floats(
                    &mut words,
                    &[radiance[0], radiance[1], radiance[2], emitter.half[1]],
                );
            }
            None => words.extend_from_slice(&[0; 16]),
        }
    }
    words
}

/// A palette's joints as rotations for the shader (ADR-814, `mesh::joint_rotations`): sixteen
/// floats a joint, laid after the palette's own matrices so a joint's are at `start + joint
/// count + joint` — its unit quaternion `[x y z w]`, then its stretch's three columns, each
/// padded to four.
pub(super) fn joint_rotation_floats(palette: &[f32]) -> Vec<f32> {
    let rotations = crate::host::gui::mesh::joint_rotations(palette);
    let mut floats = Vec::with_capacity(rotations.quaternions.len() * 16);
    for (quaternion, stretch) in rotations.quaternions.iter().zip(&rotations.stretches) {
        floats.extend_from_slice(quaternion);
        for column in 0..3 {
            floats.extend_from_slice(&stretch[column * 3..column * 3 + 3]);
            floats.push(0.0);
        }
    }
    floats
}
