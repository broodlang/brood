//! 3D meshes and the scenes that draw them (ADR-786): the MECHANISM a 3D game needs and
//! nothing more. A mesh is uploaded once (`gui-mesh`): positions, normals, texture
//! coordinates, optional tangents, the joints and weights that skin it to a skeleton, the
//! triangles, and morph targets. A `[:scene …]` op draws meshes into a pixel rect of the
//! frame: a camera (one view-projection matrix), a few directional lights over a sky/ground
//! ambient, the skeleton's joint matrices for this frame (a PALETTE), and per draw the mesh,
//! its model matrix, its morph weights and a material from a FIXED shading model. Loading a
//! glTF, posing a skeleton, a camera's maths, animation — all Brood policy above this.
//!
//! The shading model is fixed on purpose (ADR-413: a frame carries no shader code). It is
//! written ONCE here for the CPU painter, which rasterises a scene in software
//! ([`rasterize`]), and mirrored in WGSL for the GPU target (`gpu/shaders.rs`, `MESH_SRC`):
//! wrapped diffuse with a scatter tint at the terminator for skin, Blinn-Phong specular
//! normalised by its exponent, a fresnel rim, a hemispherical ambient, a tangent-space
//! normal map, alpha cut out (lashes, brows) or blended (hair), ACES tone mapping and sRGB.
//! A scene is composited into the frame as a premultiplied picture where its op stands, so
//! 2D ops before it are behind it and ops after it in front.

use std::collections::HashMap;
use std::sync::Arc;

/// The most directional lights a scene lights with; more are ignored.
pub const MAX_LIGHTS: usize = 4;
/// The most morph targets one draw blends (the heaviest, when it names more).
pub const MAX_MORPHS: usize = 8;
/// The largest side of a scene's picture, in pixels.
pub const MAX_SCENE_SIDE: u32 = 4096;
/// The most vertices one mesh may have: 80 bytes each on the GPU target stays under the
/// smallest buffer every device allows (256 MiB).
pub const MAX_VERTICES: usize = 3 << 20;
/// The most bytes of morph targets one mesh may carry: 32 bytes a vertex a target on the GPU
/// target, under the smallest storage binding every device allows (128 MiB).
pub const MAX_MORPH_BYTES: usize = 128 << 20;
/// The most indices one mesh may have (4 bytes each on the GPU target, under 256 MiB).
pub const MAX_INDICES: usize = 63 << 20;

/// One morph target: per-vertex offsets added to the positions and normals, scaled by the
/// target's weight in a draw.
#[derive(Clone, Debug, PartialEq)]
pub struct Morph {
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
}

/// A mesh as uploaded and validated: every attribute has one entry per vertex (or none for
/// the optional ones), every index names a vertex, and the triangles are whole.
#[derive(Clone, Debug, PartialEq)]
pub struct Mesh {
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    /// Tangents with the bitangent's sign in `w` (glTF's convention), for a normal map.
    pub tangents: Option<Vec<[f32; 4]>>,
    /// The four joints each vertex is skinned to (indices into a draw's palette) and their
    /// weights; empty for a rigid mesh.
    pub joints: Vec<[u16; 4]>,
    pub weights: Vec<[f32; 4]>,
    pub indices: Vec<u32>,
    pub morphs: Vec<Morph>,
}

/// A mesh's attributes as raw little-endian bytes, the way a glTF buffer holds them, before
/// [`Mesh::from_parts`] checks and unpacks them.
#[derive(Default)]
pub struct MeshParts {
    pub positions: Vec<u8>,
    pub normals: Vec<u8>,
    pub uvs: Vec<u8>,
    pub tangents: Vec<u8>,
    /// `u8` or `u16` joint indices, four a vertex, per `joints_wide`.
    pub joints: Vec<u8>,
    pub joints_wide: bool,
    pub weights: Vec<u8>,
    /// `u16` or `u32` indices, per `indices_wide`; empty for a plain triangle list (the
    /// vertices taken three at a time, as glTF allows).
    pub indices: Vec<u8>,
    pub indices_wide: bool,
    pub morphs: Vec<(Vec<u8>, Vec<u8>)>,
}

fn floats<const N: usize>(name: &str, bytes: &[u8], count: usize) -> Result<Vec<[f32; N]>, String> {
    if bytes.len() != count * N * 4 {
        return Err(format!(
            "{name}: {count} vertices need {} bytes, got {}",
            count * N * 4,
            bytes.len()
        ));
    }
    Ok(bytes
        .as_chunks::<4>()
        .0
        .as_chunks::<N>()
        .0
        .iter()
        .map(|entry| {
            let mut out = [0.0f32; N];
            for (slot, word) in out.iter_mut().zip(entry) {
                let value = f32::from_le_bytes(*word);
                *slot = if value.is_finite() { value } else { 0.0 };
            }
            out
        })
        .collect())
}

impl Mesh {
    /// The mesh `parts` describe, or why it is not one: a byte count that is not a whole
    /// number of entries per vertex, an index past the last vertex, a ragged triangle list, a
    /// mesh too big for the GPU target to hold.
    pub fn from_parts(parts: MeshParts) -> Result<Mesh, String> {
        if parts.positions.is_empty() || !parts.positions.len().is_multiple_of(12) {
            return Err(format!(
                "positions: need a whole number of 12-byte vertices, got {} bytes",
                parts.positions.len()
            ));
        }
        let count = parts.positions.len() / 12;
        if count > MAX_VERTICES {
            return Err(format!("{count} vertices is more than {MAX_VERTICES}"));
        }
        let positions = floats::<3>("positions", &parts.positions, count)?;
        let normals = if parts.normals.is_empty() {
            vec![[0.0, 0.0, 1.0]; count]
        } else {
            floats::<3>("normals", &parts.normals, count)?
        };
        let uvs = if parts.uvs.is_empty() {
            vec![[0.0, 0.0]; count]
        } else {
            floats::<2>("uvs", &parts.uvs, count)?
        };
        let tangents = if parts.tangents.is_empty() {
            None
        } else {
            Some(floats::<4>("tangents", &parts.tangents, count)?)
        };
        let (joints, weights) = if parts.joints.is_empty() && parts.weights.is_empty() {
            (Vec::new(), Vec::new())
        } else {
            let width = if parts.joints_wide { 2 } else { 1 };
            if parts.joints.len() != count * 4 * width {
                return Err(format!(
                    "joints: {count} vertices need {} bytes, got {}",
                    count * 4 * width,
                    parts.joints.len()
                ));
            }
            let joints = parts
                .joints
                .chunks_exact(4 * width)
                .map(|entry| {
                    let mut out = [0u16; 4];
                    for (slot, joint) in out.iter_mut().enumerate() {
                        *joint = if parts.joints_wide {
                            u16::from_le_bytes([entry[slot * 2], entry[slot * 2 + 1]])
                        } else {
                            entry[slot] as u16
                        };
                    }
                    out
                })
                .collect();
            let mut weights = floats::<4>("weights", &parts.weights, count)?;
            for weight in &mut weights {
                let total: f32 = weight.iter().map(|share| share.max(0.0)).sum();
                for share in weight.iter_mut() {
                    *share = if total > 0.0 {
                        share.max(0.0) / total
                    } else {
                        0.0
                    };
                }
            }
            (joints, weights)
        };
        let indices: Vec<u32> = if parts.indices.is_empty() {
            if !count.is_multiple_of(3) {
                return Err(format!(
                    "indices: none given, so the {count} vertices must be whole triangles"
                ));
            }
            (0..count as u32).collect()
        } else {
            let width = if parts.indices_wide { 4 } else { 2 };
            if !parts.indices.len().is_multiple_of(width * 3) {
                return Err(format!(
                    "indices: need whole triangles of {width}-byte indices, got {} bytes",
                    parts.indices.len()
                ));
            }
            parts
                .indices
                .chunks_exact(width)
                .map(|entry| {
                    if parts.indices_wide {
                        u32::from_le_bytes([entry[0], entry[1], entry[2], entry[3]])
                    } else {
                        u16::from_le_bytes([entry[0], entry[1]]) as u32
                    }
                })
                .collect()
        };
        if indices.len() > MAX_INDICES {
            return Err(format!(
                "{} indices is more than {MAX_INDICES}",
                indices.len()
            ));
        }
        if let Some(bad) = indices.iter().find(|&&index| index as usize >= count) {
            return Err(format!("indices: {bad} names no vertex of {count}"));
        }
        let morph_bytes = parts.morphs.len() * count * 32;
        if morph_bytes > MAX_MORPH_BYTES {
            return Err(format!(
                "{} morph targets of {count} vertices need {morph_bytes} bytes, more than {MAX_MORPH_BYTES}",
                parts.morphs.len()
            ));
        }
        let mut morphs = Vec::with_capacity(parts.morphs.len());
        for (target, (morph_positions, morph_normals)) in parts.morphs.iter().enumerate() {
            let positions =
                floats::<3>(&format!("morph {target} positions"), morph_positions, count)?;
            let normals = if morph_normals.is_empty() {
                vec![[0.0; 3]; count]
            } else {
                floats::<3>(&format!("morph {target} normals"), morph_normals, count)?
            };
            morphs.push(Morph { positions, normals });
        }
        Ok(Mesh {
            positions,
            normals,
            uvs,
            tangents,
            joints,
            weights,
            indices,
            morphs,
        })
    }
}

/// How a surface is shaded and blended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Surface {
    /// Wrapped diffuse with the scatter colour at the terminator (light under the skin).
    Skin,
    /// Plain diffuse.
    Standard,
    /// Standard, with texels under the material's `cutoff` alpha not drawn (lashes, brows).
    Cutout,
    /// Standard, blended over what is behind it by its alpha, after every other surface and
    /// without writing depth (hair cards).
    Blend,
}

/// A draw's material: the fixed shading model's parameters.
#[derive(Clone, Debug, PartialEq)]
pub struct Material {
    pub surface: Surface,
    /// The base colour texture (a `gui-texture` handle; sRGB), multiplied by `color`.
    pub base: Option<u32>,
    /// A tangent-space normal map (a `gui-texture` handle; linear), used when the mesh has
    /// tangents.
    pub normal: Option<u32>,
    /// Linear RGBA factor on the base colour.
    pub color: [f32; 4],
    /// 0 (mirror) … 1 (chalk).
    pub roughness: f32,
    /// The specular reflection's strength.
    pub specular: f32,
    /// How far diffuse light wraps past the terminator, 0 … 1 (skin).
    pub wrap: f32,
    /// The colour the wrapped light takes (skin's red where light goes under it).
    pub scatter: [f32; 3],
    /// The fresnel rim's strength, lit by the sky.
    pub rim: f32,
    /// The alpha under which a cutout texel is not drawn.
    pub cutoff: f32,
    /// Draw back faces too (their normals turned to face the viewer).
    pub double_sided: bool,
}

impl Default for Material {
    fn default() -> Self {
        Material {
            surface: Surface::Standard,
            base: None,
            normal: None,
            color: [1.0, 1.0, 1.0, 1.0],
            roughness: 0.6,
            specular: 0.3,
            wrap: 0.0,
            scatter: [0.0, 0.0, 0.0],
            rim: 0.0,
            cutoff: 0.5,
            double_sided: false,
        }
    }
}

/// A directional light: the direction its light TRAVELS (from the light into the scene) and
/// its linear colour, intensity included.
#[derive(Clone, Debug, PartialEq)]
pub struct Light {
    pub direction: [f32; 3],
    pub color: [f32; 3],
}

/// One mesh drawn in a scene.
#[derive(Clone, Debug, PartialEq)]
pub struct Draw {
    pub mesh: u32,
    /// Column-major model matrix (glTF's layout).
    pub model: [f32; 16],
    /// Which of the scene's palettes skins it; `None` draws it rigid.
    pub palette: Option<usize>,
    /// `(target, weight)` pairs, at most [`MAX_MORPHS`] of them, heaviest first.
    pub morphs: Vec<(u32, f32)>,
    pub material: Material,
}

/// A `[:scene …]` op: meshes drawn into the pixel rect `x y w h` of the frame.
#[derive(Clone, Debug, PartialEq)]
pub struct Scene {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    /// Column-major view-projection matrix, clip depth 0 (near) … 1 (far).
    pub view_proj: [f32; 16],
    /// The camera's position, for the specular and the rim.
    pub eye: [f32; 3],
    pub lights: Vec<Light>,
    /// The ambient light from above and from below (linear).
    pub sky: [f32; 3],
    pub ground: [f32; 3],
    /// Scales the light before tone mapping.
    pub exposure: f32,
    /// What the rect shows where no mesh is: straight RGBA 0 … 1, transparent by default.
    pub background: [f32; 4],
    /// Joint matrices (column-major, 16 floats each), one list per skeleton.
    pub palettes: Vec<Arc<[f32]>>,
    pub draws: Vec<Draw>,
}

impl Scene {
    /// The picture's size in whole pixels.
    pub fn pixel_size(&self) -> (u32, u32) {
        let side = |extent: f32| {
            if extent.is_finite() {
                (extent.round().max(1.0) as u32).min(MAX_SCENE_SIDE)
            } else {
                1
            }
        };
        (side(self.w), side(self.h))
    }

    /// The draws in the order they are drawn: every non-blended one in the scene's order,
    /// then the blended ones in theirs.
    pub fn draw_order(&self) -> impl Iterator<Item = &Draw> {
        let solid = self
            .draws
            .iter()
            .filter(|draw| draw.material.surface != Surface::Blend);
        let blended = self
            .draws
            .iter()
            .filter(|draw| draw.material.surface == Surface::Blend);
        solid.chain(blended)
    }

    /// The palette `draw` is skinned by and how many joints it holds — `None` for a rigid draw
    /// (no palette named, or one holding no joint). Both painters skin only by joints below
    /// that count.
    pub fn palette_of(&self, draw: &Draw) -> Option<&[f32]> {
        let palette = draw.palette.and_then(|index| self.palettes.get(index))?;
        (palette.len() >= 16).then_some(&palette[..])
    }
}

// --- the shading model (mirrored in `gpu/shaders.rs`, MESH_SRC) -------------------------------

fn dot(left: [f32; 3], right: [f32; 3]) -> f32 {
    left[0] * right[0] + left[1] * right[1] + left[2] * right[2]
}

/// `vector` made unit length, or +z when it has none — the guard the shader mirrors
/// (`safe_normalize`).
fn normalize(vector: [f32; 3]) -> [f32; 3] {
    let length = dot(vector, vector).sqrt();
    if length > 1e-12 {
        [vector[0] / length, vector[1] / length, vector[2] / length]
    } else {
        [0.0, 0.0, 1.0]
    }
}

fn cross(left: [f32; 3], right: [f32; 3]) -> [f32; 3] {
    [
        left[1] * right[2] - left[2] * right[1],
        left[2] * right[0] - left[0] * right[2],
        left[0] * right[1] - left[1] * right[0],
    ]
}

/// An sRGB-encoded channel (0 … 1) in linear light.
pub fn srgb_to_linear(channel: f32) -> f32 {
    if channel <= 0.04045 {
        channel / 12.92
    } else {
        ((channel + 0.055) / 1.055).powf(2.4)
    }
}

/// A linear channel (0 … 1) sRGB-encoded.
pub fn linear_to_srgb(channel: f32) -> f32 {
    let channel = channel.clamp(0.0, 1.0);
    if channel <= 0.0031308 {
        channel * 12.92
    } else {
        1.055 * channel.powf(1.0 / 2.4) - 0.055
    }
}

/// The specular exponent for a roughness.
pub fn shininess(roughness: f32) -> f32 {
    let clamped = roughness.clamp(0.04, 1.0);
    (2.0 / (clamped * clamped * clamped * clamped) - 2.0).clamp(1.0, 4096.0)
}

/// The light leaving a surface point toward the eye, linear and before exposure: `base` its
/// linear colour, `normal` its unit normal facing the viewer's side, `view` the unit
/// direction from it to the eye.
pub fn shade(
    scene: &Scene,
    material: &Material,
    base: [f32; 3],
    normal: [f32; 3],
    view: [f32; 3],
) -> [f32; 3] {
    let up = normal[1] * 0.5 + 0.5;
    let mut out = [0.0f32; 3];
    for channel in 0..3 {
        out[channel] = base[channel]
            * (scene.ground[channel] + (scene.sky[channel] - scene.ground[channel]) * up);
    }
    let exponent = shininess(material.roughness);
    let normalisation = (exponent + 8.0) / (8.0 * std::f32::consts::PI);
    for light in scene.lights.iter().take(MAX_LIGHTS) {
        let to_light = normalize([
            -light.direction[0],
            -light.direction[1],
            -light.direction[2],
        ]);
        let facing = dot(normal, to_light);
        let lambert = facing.max(0.0);
        let half = normalize([
            to_light[0] + view[0],
            to_light[1] + view[1],
            to_light[2] + view[2],
        ]);
        let specular =
            material.specular * normalisation * dot(normal, half).max(0.0).powf(exponent) * lambert;
        for channel in 0..3 {
            let diffuse = if material.surface == Surface::Skin {
                let wrapped = ((facing + material.wrap) / (1.0 + material.wrap)).clamp(0.0, 1.0);
                lambert + material.scatter[channel] * (wrapped - lambert)
            } else {
                lambert
            };
            out[channel] += light.color[channel] * (base[channel] * diffuse + specular);
        }
    }
    let rim = material.rim * (1.0 - dot(normal, view).max(0.0)).powi(3);
    for channel in 0..3 {
        out[channel] += rim * scene.sky[channel];
    }
    out
}

/// Linear light after `exposure`, tone mapped (ACES, Narkowicz's fit) and sRGB-encoded.
pub fn tone(color: [f32; 3], exposure: f32) -> [f32; 3] {
    let map = |channel: f32| {
        let exposed = (channel * exposure).max(0.0);
        let mapped =
            (exposed * (2.51 * exposed + 0.03)) / (exposed * (2.43 * exposed + 0.59) + 0.14);
        linear_to_srgb(mapped)
    };
    [map(color[0]), map(color[1]), map(color[2])]
}

// --- the CPU painter's rasteriser ---------------------------------------------------------------

/// A texture as the rasteriser samples it: straight-alpha RGBA bytes.
pub struct TextureRef<'a> {
    pub w: u32,
    pub h: u32,
    pub rgba: &'a [u8],
}

/// A texel bilinearly filtered at `(u, v)` (repeating), each channel 0 … 1, still encoded.
fn sample(texture: &TextureRef, u: f32, v: f32) -> [f32; 4] {
    if texture.w == 0 || texture.h == 0 || texture.rgba.len() < (texture.w * texture.h * 4) as usize
    {
        return [1.0; 4];
    }
    let (width, height) = (texture.w as i64, texture.h as i64);
    let across = u.rem_euclid(1.0) * width as f32 - 0.5;
    let down = v.rem_euclid(1.0) * height as f32 - 0.5;
    let (left, top) = (across.floor(), down.floor());
    let (fraction_x, fraction_y) = (across - left, down - top);
    let texel = |column: i64, row: i64| {
        let column = column.rem_euclid(width) as usize;
        let row = row.rem_euclid(height) as usize;
        let start = (row * texture.w as usize + column) * 4;
        let pixel = &texture.rgba[start..start + 4];
        let alpha = pixel[3] as f32 / 255.0;
        // filtered premultiplied, so a transparent texel's colour does not bleed in
        [
            pixel[0] as f32 / 255.0 * alpha,
            pixel[1] as f32 / 255.0 * alpha,
            pixel[2] as f32 / 255.0 * alpha,
            alpha,
        ]
    };
    let (left, top) = (left as i64, top as i64);
    let (top_left, top_right) = (texel(left, top), texel(left + 1, top));
    let (bottom_left, bottom_right) = (texel(left, top + 1), texel(left + 1, top + 1));
    let mut out = [0.0f32; 4];
    for channel in 0..4 {
        let upper = top_left[channel] + (top_right[channel] - top_left[channel]) * fraction_x;
        let lower =
            bottom_left[channel] + (bottom_right[channel] - bottom_left[channel]) * fraction_x;
        out[channel] = upper + (lower - upper) * fraction_y;
    }
    let coverage = out[3];
    if coverage > 1e-6 {
        for value in out.iter_mut().take(3) {
            *value /= coverage;
        }
    }
    out
}

fn transform(matrix: &[f32; 16], point: [f32; 3], w: f32) -> [f32; 4] {
    [
        matrix[0] * point[0] + matrix[4] * point[1] + matrix[8] * point[2] + matrix[12] * w,
        matrix[1] * point[0] + matrix[5] * point[1] + matrix[9] * point[2] + matrix[13] * w,
        matrix[2] * point[0] + matrix[6] * point[1] + matrix[10] * point[2] + matrix[14] * w,
        matrix[3] * point[0] + matrix[7] * point[1] + matrix[11] * point[2] + matrix[15] * w,
    ]
}

fn multiply(left: &[f32; 16], right: &[f32; 16]) -> [f32; 16] {
    let mut out = [0.0f32; 16];
    for column in 0..4 {
        for row in 0..4 {
            out[column * 4 + row] = (0..4)
                .map(|term| left[term * 4 + row] * right[column * 4 + term])
                .sum();
        }
    }
    out
}

/// The skinning matrix of vertex `index` — the weighted sum of its joints' palette matrices,
/// a joint past the palette's end skipped — or `None` for a rigid draw or a vertex no joint
/// of the palette moves. The shader skins the same way (`vs_mesh`).
pub fn skin_matrix(mesh: &Mesh, palette: Option<&[f32]>, index: usize) -> Option<[f32; 16]> {
    let palette = palette?;
    if mesh.joints.is_empty() {
        return None;
    }
    let joints = mesh.joints[index];
    let weights = mesh.weights[index];
    let mut out = [0.0f32; 16];
    let mut any = false;
    for (joint, weight) in joints.iter().zip(weights) {
        let start = *joint as usize * 16;
        if weight <= 0.0 || start + 16 > palette.len() {
            continue;
        }
        any = true;
        for (slot, value) in out.iter_mut().zip(&palette[start..start + 16]) {
            *slot += weight * value;
        }
    }
    any.then_some(out)
}

/// A vertex ready to rasterise: its clip position and what the fragment interpolates.
#[derive(Clone, Copy)]
struct ClipVertex {
    clip: [f32; 4],
    world: [f32; 3],
    normal: [f32; 3],
    tangent: [f32; 4],
    uv: [f32; 2],
}

fn lerp_vertex(from: &ClipVertex, to: &ClipVertex, amount: f32) -> ClipVertex {
    let mix = |start: f32, end: f32| start + (end - start) * amount;
    let mix_n = |start: &[f32], end: &[f32], out: &mut [f32]| {
        for ((slot, begin), finish) in out.iter_mut().zip(start).zip(end) {
            *slot = mix(*begin, *finish);
        }
    };
    let mut vertex = *from;
    mix_n(&from.clip, &to.clip, &mut vertex.clip);
    mix_n(&from.world, &to.world, &mut vertex.world);
    mix_n(&from.normal, &to.normal, &mut vertex.normal);
    mix_n(
        &from.tangent[..3],
        &to.tangent[..3],
        &mut vertex.tangent[..3],
    );
    mix_n(&from.uv, &to.uv, &mut vertex.uv);
    vertex
}

/// The smallest clip `w` a vertex may have: the near plane the rasteriser clips at, as the GPU
/// clips at depth 0.
const NEAR_W: f32 = 1e-5;

/// `triangle` clipped to the half-space in front of the camera (`w ≥ NEAR_W` and depth ≥ 0):
/// none, or a convex polygon of three or four vertices — so a floor the camera stands on, or a
/// room it is inside, keeps the part in front of it, as on the GPU.
fn clip_near(triangle: [ClipVertex; 3]) -> Vec<ClipVertex> {
    // the distance inside each plane: w − NEAR_W, and z (clip depth 0 … w)
    let planes: [fn(&ClipVertex) -> f32; 2] =
        [|vertex| vertex.clip[3] - NEAR_W, |vertex| vertex.clip[2]];
    let mut polygon: Vec<ClipVertex> = triangle.to_vec();
    for inside in planes {
        if polygon.iter().all(|vertex| inside(vertex) >= 0.0) {
            continue;
        }
        let mut clipped = Vec::with_capacity(polygon.len() + 1);
        for (position, current) in polygon.iter().enumerate() {
            let next = &polygon[(position + 1) % polygon.len()];
            let (here, there) = (inside(current), inside(next));
            if here >= 0.0 {
                clipped.push(*current);
            }
            if (here >= 0.0) != (there >= 0.0) {
                clipped.push(lerp_vertex(current, next, here / (here - there)));
            }
        }
        polygon = clipped;
        if polygon.len() < 3 {
            return Vec::new();
        }
    }
    polygon
}

/// A scene's colour and depth as the rasteriser fills them, at the supersampled size.
struct Canvas {
    width: usize,
    height: usize,
    /// Premultiplied, sRGB-encoded — the space the GPU target blends in.
    color: Vec<[f32; 4]>,
    depth: Vec<f32>,
}

/// One draw's state the fragments of its triangles read.
struct DrawContext<'a> {
    scene: &'a Scene,
    material: &'a Material,
    base_texture: Option<TextureRef<'a>>,
    normal_texture: Option<TextureRef<'a>>,
}

/// Rasterise one triangle (already clipped in front of the camera) into `canvas`.
fn fill_triangle(canvas: &mut Canvas, context: &DrawContext, corners: [&ClipVertex; 3]) {
    let material = context.material;
    let (width, height) = (canvas.width as f32, canvas.height as f32);
    let to_device = |vertex: &ClipVertex| {
        let inverse_w = 1.0 / vertex.clip[3];
        [
            vertex.clip[0] * inverse_w,
            vertex.clip[1] * inverse_w,
            vertex.clip[2] * inverse_w,
        ]
    };
    let device = [
        to_device(corners[0]),
        to_device(corners[1]),
        to_device(corners[2]),
    ];
    // counter-clockwise in normalised device coordinates faces the viewer (glTF's winding, and
    // the GPU pipeline's front face)
    let area = (device[1][0] - device[0][0]) * (device[2][1] - device[0][1])
        - (device[2][0] - device[0][0]) * (device[1][1] - device[0][1]);
    let front = area > 0.0;
    if area == 0.0 || !area.is_finite() || (!front && !material.double_sided) {
        return;
    }
    // screen space in f64: a triangle clipped just in front of the camera reaches hundreds of
    // thousands of pixels off the picture, where f32 edge functions cancel to noise
    let (width, height) = (f64::from(width), f64::from(height));
    let to_screen = |point: [f32; 3]| {
        [
            (f64::from(point[0]) * 0.5 + 0.5) * width,
            (0.5 - f64::from(point[1]) * 0.5) * height,
            f64::from(point[2]),
        ]
    };
    let screen = [
        to_screen(device[0]),
        to_screen(device[1]),
        to_screen(device[2]),
    ];
    let min_x = screen
        .iter()
        .map(|point| point[0])
        .fold(f64::INFINITY, f64::min)
        .floor()
        .max(0.0) as usize;
    let max_x = screen
        .iter()
        .map(|point| point[0])
        .fold(f64::NEG_INFINITY, f64::max)
        .ceil()
        .min(width) as usize;
    let min_y = screen
        .iter()
        .map(|point| point[1])
        .fold(f64::INFINITY, f64::min)
        .floor()
        .max(0.0) as usize;
    let max_y = screen
        .iter()
        .map(|point| point[1])
        .fold(f64::NEG_INFINITY, f64::max)
        .ceil()
        .min(height) as usize;
    if min_x >= max_x || min_y >= max_y {
        return;
    }
    let edge = |from: [f64; 3], to: [f64; 3], x: f64, y: f64| {
        (to[0] - from[0]) * (y - from[1]) - (to[1] - from[1]) * (x - from[0])
    };
    let total = edge(screen[0], screen[1], screen[2][0], screen[2][1]);
    if total.abs() < 1e-12 {
        return;
    }
    let inverse_w = [
        1.0 / corners[0].clip[3],
        1.0 / corners[1].clip[3],
        1.0 / corners[2].clip[3],
    ];
    let sign = if corners[0].tangent[3] < 0.0 {
        -1.0
    } else {
        1.0
    };
    let blend = material.surface == Surface::Blend;
    let scene = context.scene;
    for row in min_y..max_y {
        for column in min_x..max_x {
            let (x, y) = (column as f64 + 0.5, row as f64 + 0.5);
            let first = edge(screen[1], screen[2], x, y) / total;
            let second = edge(screen[2], screen[0], x, y) / total;
            let third = 1.0 - first - second;
            if first < 0.0 || second < 0.0 || third < 0.0 {
                continue;
            }
            let depth =
                (first * screen[0][2] + second * screen[1][2] + third * screen[2][2]) as f32;
            let (first, second, third) = (first as f32, second as f32, third as f32);
            let slot = row * canvas.width + column;
            if !(0.0..=1.0).contains(&depth) || depth >= canvas.depth[slot] {
                continue;
            }
            // perspective-correct weights for the attributes
            let weights = [
                first * inverse_w[0],
                second * inverse_w[1],
                third * inverse_w[2],
            ];
            let sum = weights[0] + weights[1] + weights[2];
            let weights = [weights[0] / sum, weights[1] / sum, weights[2] / sum];
            let blend3 = |pick: fn(&ClipVertex) -> [f32; 3]| {
                let values = [pick(corners[0]), pick(corners[1]), pick(corners[2])];
                [
                    values[0][0] * weights[0]
                        + values[1][0] * weights[1]
                        + values[2][0] * weights[2],
                    values[0][1] * weights[0]
                        + values[1][1] * weights[1]
                        + values[2][1] * weights[2],
                    values[0][2] * weights[0]
                        + values[1][2] * weights[1]
                        + values[2][2] * weights[2],
                ]
            };
            let uv = [
                corners[0].uv[0] * weights[0]
                    + corners[1].uv[0] * weights[1]
                    + corners[2].uv[0] * weights[2],
                corners[0].uv[1] * weights[0]
                    + corners[1].uv[1] * weights[1]
                    + corners[2].uv[1] * weights[2],
            ];
            let texel = context
                .base_texture
                .as_ref()
                .map_or([1.0; 4], |texture| sample(texture, uv[0], uv[1]));
            let mut alpha = texel[3] * material.color[3];
            match material.surface {
                Surface::Cutout => {
                    if alpha < material.cutoff {
                        continue;
                    }
                    alpha = 1.0;
                }
                Surface::Blend => {
                    if alpha <= 0.002 {
                        continue;
                    }
                }
                Surface::Skin | Surface::Standard => alpha = 1.0,
            }
            let base = [
                srgb_to_linear(texel[0]) * material.color[0],
                srgb_to_linear(texel[1]) * material.color[1],
                srgb_to_linear(texel[2]) * material.color[2],
            ];
            let world = blend3(|vertex| vertex.world);
            let mut normal = normalize(blend3(|vertex| vertex.normal));
            if !front {
                normal = [-normal[0], -normal[1], -normal[2]];
            }
            if let Some(map) = context.normal_texture.as_ref() {
                let tangent = normalize(blend3(|vertex| {
                    [vertex.tangent[0], vertex.tangent[1], vertex.tangent[2]]
                }));
                let bitangent = cross(normal, tangent);
                let texel = sample(map, uv[0], uv[1]);
                let local = [
                    texel[0] * 2.0 - 1.0,
                    texel[1] * 2.0 - 1.0,
                    texel[2] * 2.0 - 1.0,
                ];
                normal = normalize([
                    tangent[0] * local[0] + sign * bitangent[0] * local[1] + normal[0] * local[2],
                    tangent[1] * local[0] + sign * bitangent[1] * local[1] + normal[1] * local[2],
                    tangent[2] * local[0] + sign * bitangent[2] * local[1] + normal[2] * local[2],
                ]);
            }
            let eye = scene.eye;
            let view = normalize([eye[0] - world[0], eye[1] - world[1], eye[2] - world[2]]);
            let lit = tone(shade(scene, material, base, normal, view), scene.exposure);
            let under = canvas.color[slot];
            canvas.color[slot] = [
                lit[0] * alpha + under[0] * (1.0 - alpha),
                lit[1] * alpha + under[1] * (1.0 - alpha),
                lit[2] * alpha + under[2] * (1.0 - alpha),
                alpha + under[3] * (1.0 - alpha),
            ];
            if !blend {
                canvas.depth[slot] = depth;
            }
        }
    }
}

/// The vertices of `mesh` drawn by `draw`: morphed, skinned, moved by the model matrix and seen
/// through the scene's camera.
fn pose_vertices(scene: &Scene, mesh: &Mesh, draw: &Draw) -> Vec<ClipVertex> {
    let palette = scene.palette_of(draw);
    let morphs: Vec<(&Morph, f32)> = draw
        .morphs
        .iter()
        .take(MAX_MORPHS)
        .filter_map(|&(target, weight)| {
            mesh.morphs
                .get(target as usize)
                .map(|morph| (morph, weight))
        })
        .filter(|(_, weight)| *weight != 0.0)
        .collect();
    (0..mesh.positions.len())
        .map(|index| {
            let mut position = mesh.positions[index];
            let mut normal = mesh.normals[index];
            for (morph, weight) in &morphs {
                for axis in 0..3 {
                    position[axis] += weight * morph.positions[index][axis];
                    normal[axis] += weight * morph.normals[index][axis];
                }
            }
            let tangent = mesh
                .tangents
                .as_ref()
                .map_or([1.0, 0.0, 0.0, 1.0], |tangents| tangents[index]);
            let matrix = match skin_matrix(mesh, palette, index) {
                Some(skin) => multiply(&draw.model, &skin),
                None => draw.model,
            };
            let world = transform(&matrix, position, 1.0);
            let turned_normal = transform(&matrix, normal, 0.0);
            let turned_tangent = transform(&matrix, [tangent[0], tangent[1], tangent[2]], 0.0);
            ClipVertex {
                clip: transform(&scene.view_proj, [world[0], world[1], world[2]], 1.0),
                world: [world[0], world[1], world[2]],
                normal: normalize([turned_normal[0], turned_normal[1], turned_normal[2]]),
                tangent: {
                    let unit = normalize([turned_tangent[0], turned_tangent[1], turned_tangent[2]]);
                    [unit[0], unit[1], unit[2], tangent[3]]
                },
                uv: mesh.uvs[index],
            }
        })
        .collect()
}

/// How many times finer than the picture the CPU painter rasterises a scene of at most
/// `pixels` pixels and averages down: the anti-aliasing the GPU's 4× multisampling gives, for
/// a scene small enough that four samples a pixel stay cheap; one sample for a bigger one, so
/// a full-screen scene costs 20 bytes a pixel, not 80.
fn supersample_for(pixels: usize) -> usize {
    if pixels <= 1 << 20 { 2 } else { 1 }
}

/// `scene` rasterised in software: its picture, `w`×`h` straight-alpha RGBA bytes (sRGB),
/// what the CPU painter draws where the op stands. `meshes` and `textures` look up handles;
/// a draw naming a mesh that is not there draws nothing, a missing texture samples white.
pub fn rasterize<'t>(
    scene: &Scene,
    meshes: &dyn Fn(u32) -> Option<Arc<Mesh>>,
    textures: &dyn Fn(u32) -> Option<TextureRef<'t>>,
) -> (u32, u32, Vec<u8>) {
    let (out_width, out_height) = scene.pixel_size();
    let supersample = supersample_for(out_width as usize * out_height as usize);
    let (width, height) = (
        out_width as usize * supersample,
        out_height as usize * supersample,
    );
    let background = scene.background;
    let clear = [
        background[0].clamp(0.0, 1.0) * background[3],
        background[1].clamp(0.0, 1.0) * background[3],
        background[2].clamp(0.0, 1.0) * background[3],
        background[3].clamp(0.0, 1.0),
    ];
    let mut canvas = Canvas {
        width,
        height,
        color: vec![clear; width * height],
        depth: vec![1.0f32; width * height],
    };
    for draw in scene.draw_order() {
        let Some(mesh) = meshes(draw.mesh) else {
            continue;
        };
        let vertices = pose_vertices(scene, &mesh, draw);
        let material = &draw.material;
        let context = DrawContext {
            scene,
            material,
            base_texture: material.base.and_then(textures),
            normal_texture: if mesh.tangents.is_some() {
                material.normal.and_then(textures)
            } else {
                None
            },
        };
        for triangle in mesh.indices.as_chunks::<3>().0 {
            let corners = [
                vertices[triangle[0] as usize],
                vertices[triangle[1] as usize],
                vertices[triangle[2] as usize],
            ];
            let polygon = clip_near(corners);
            // a convex polygon, as a fan from its first vertex
            for fan in 1..polygon.len().saturating_sub(1) {
                fill_triangle(
                    &mut canvas,
                    &context,
                    [&polygon[0], &polygon[fan], &polygon[fan + 1]],
                );
            }
        }
    }
    // averaged down to the picture's pixels, premultiplied, then straight alpha
    let mut rgba = vec![0u8; out_width as usize * out_height as usize * 4];
    let samples = (supersample * supersample) as f32;
    for out_row in 0..out_height as usize {
        for out_column in 0..out_width as usize {
            let mut sum = [0.0f32; 4];
            for sub_row in 0..supersample {
                for sub_column in 0..supersample {
                    let pixel = canvas.color[(out_row * supersample + sub_row) * width
                        + out_column * supersample
                        + sub_column];
                    for channel in 0..4 {
                        sum[channel] += pixel[channel];
                    }
                }
            }
            let alpha = sum[3] / samples;
            let start = (out_row * out_width as usize + out_column) * 4;
            if alpha > 1e-6 {
                for channel in 0..3 {
                    rgba[start + channel] =
                        ((sum[channel] / samples / alpha).clamp(0.0, 1.0) * 255.0).round() as u8;
                }
            }
            rgba[start + 3] = (alpha.clamp(0.0, 1.0) * 255.0).round() as u8;
        }
    }
    (out_width, out_height, rgba)
}

/// The meshes a window keeps, by handle — shared by both painters (the CPU one rasterises
/// from them; a GPU window keeps them too, for `gui/capture`, which paints in software).
#[derive(Default)]
pub struct MeshBook {
    meshes: HashMap<u32, Arc<Mesh>>,
}

impl MeshBook {
    pub fn insert(&mut self, handle: u32, mesh: Arc<Mesh>) {
        self.meshes.insert(handle, mesh);
    }
    pub fn remove(&mut self, handle: u32) {
        self.meshes.remove(&handle);
    }
    pub fn get(&self, handle: u32) -> Option<Arc<Mesh>> {
        self.meshes.get(&handle).cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bytes_of(values: &[f32]) -> Vec<u8> {
        values
            .iter()
            .flat_map(|value| value.to_le_bytes())
            .collect()
    }

    fn identity() -> [f32; 16] {
        [
            1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
        ]
    }

    /// A triangle facing +z, covering the middle of clip space.
    fn triangle() -> Mesh {
        Mesh::from_parts(MeshParts {
            positions: bytes_of(&[-0.8, -0.8, 0.5, 0.8, -0.8, 0.5, 0.0, 0.8, 0.5]),
            normals: bytes_of(&[0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0]),
            indices: [0u16, 1, 2]
                .iter()
                .flat_map(|index| index.to_le_bytes())
                .collect(),
            ..MeshParts::default()
        })
        .expect("a triangle")
    }

    fn scene(draw: Draw) -> Scene {
        Scene {
            x: 0.0,
            y: 0.0,
            w: 16.0,
            h: 16.0,
            view_proj: identity(),
            eye: [0.0, 0.0, -10.0],
            lights: vec![Light {
                direction: [0.0, 0.0, -1.0],
                color: [1.0, 1.0, 1.0],
            }],
            sky: [0.1, 0.1, 0.1],
            ground: [0.1, 0.1, 0.1],
            exposure: 1.0,
            background: [0.0, 0.0, 0.0, 0.0],
            palettes: vec![],
            draws: vec![draw],
        }
    }

    fn draw() -> Draw {
        Draw {
            mesh: 1,
            model: identity(),
            palette: None,
            morphs: vec![],
            material: Material::default(),
        }
    }

    fn alpha_at(rgba: &[u8], column: usize, row: usize) -> u8 {
        rgba[(row * 16 + column) * 4 + 3]
    }

    #[test]
    fn bad_parts_are_refused() {
        let ragged = MeshParts {
            positions: vec![0; 11],
            ..MeshParts::default()
        };
        assert!(Mesh::from_parts(ragged).is_err());
        let out_of_range = MeshParts {
            positions: bytes_of(&[0.0; 9]),
            indices: [0u16, 1, 3]
                .iter()
                .flat_map(|index| index.to_le_bytes())
                .collect(),
            ..MeshParts::default()
        };
        assert!(
            Mesh::from_parts(out_of_range)
                .unwrap_err()
                .contains("names no vertex")
        );
        // enough targets of a big mesh to pass the GPU's storage limit: refused before a byte
        // of them is read
        let vertices = 3 << 15;
        let too_many_morphs = MeshParts {
            positions: vec![0; vertices * 12],
            morphs: vec![(Vec::new(), Vec::new()); MAX_MORPH_BYTES / (vertices * 32) + 1],
            ..MeshParts::default()
        };
        assert!(
            Mesh::from_parts(too_many_morphs)
                .unwrap_err()
                .contains("morph targets")
        );
    }

    #[test]
    fn a_mesh_without_indices_is_a_triangle_list() {
        let listed = Mesh::from_parts(MeshParts {
            positions: bytes_of(&[0.0; 9]),
            ..MeshParts::default()
        })
        .expect("three vertices are one triangle");
        assert_eq!(listed.indices, vec![0, 1, 2]);
        let ragged = MeshParts {
            positions: bytes_of(&[0.0; 6]),
            ..MeshParts::default()
        };
        assert!(
            Mesh::from_parts(ragged).is_err(),
            "two vertices make no triangle"
        );
    }

    #[test]
    fn a_facing_triangle_covers_the_middle_and_leaves_the_corners() {
        let mesh = Arc::new(triangle());
        let (width, height, rgba) = rasterize(&scene(draw()), &|_| Some(mesh.clone()), &|_| None);
        assert_eq!((width, height), (16, 16));
        assert_eq!(alpha_at(&rgba, 8, 9), 255, "the middle is covered");
        assert_eq!(alpha_at(&rgba, 0, 0), 0, "a corner is the background");
    }

    #[test]
    fn a_back_face_is_culled_unless_double_sided() {
        let mesh = Arc::new(triangle());
        let mut turned = draw();
        // mirrored in x: the winding reverses, so the triangle faces away
        turned.model[0] = -1.0;
        let (_, _, rgba) = rasterize(&scene(turned.clone()), &|_| Some(mesh.clone()), &|_| None);
        assert_eq!(alpha_at(&rgba, 8, 9), 0, "a back face draws nothing");
        turned.material.double_sided = true;
        let (_, _, rgba) = rasterize(&scene(turned), &|_| Some(mesh.clone()), &|_| None);
        assert_eq!(alpha_at(&rgba, 8, 9), 255, "double-sided, it draws");
    }

    #[test]
    fn a_triangle_crossing_the_camera_keeps_the_part_in_front() {
        // a floor below the camera running behind it: one corner behind (w < 0 through a matrix
        // that copies z into w). The GPU clips it and draws the floor's near part across the
        // bottom of the picture, and so must the rasteriser — which once dropped it whole
        let mesh = Arc::new(
            Mesh::from_parts(MeshParts {
                positions: bytes_of(&[-0.8, -0.25, 0.5, 0.8, -0.25, 0.5, 0.0, -0.25, -0.5]),
                ..MeshParts::default()
            })
            .unwrap(),
        );
        let mut looking = scene(Draw {
            material: Material {
                double_sided: true,
                ..Material::default()
            },
            ..draw()
        });
        // clip w = z, clip z = z / 2: depth 0.5 everywhere in front of the camera
        looking.view_proj = [
            1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.5, 1.0, 0.0, 0.0, 0.0, 0.0,
        ];
        let (_, _, rgba) = rasterize(&looking, &|_| Some(mesh.clone()), &|_| None);
        assert_eq!(
            alpha_at(&rgba, 8, 15),
            255,
            "the floor in front of the camera is drawn"
        );
        assert_eq!(alpha_at(&rgba, 8, 2), 0, "and nothing above the horizon");
    }

    #[test]
    fn a_palette_moves_a_skinned_vertex_and_a_joint_past_it_is_skipped() {
        let mut parts = MeshParts {
            positions: bytes_of(&[-0.8, -0.8, 0.5, 0.8, -0.8, 0.5, 0.0, 0.8, 0.5]),
            ..MeshParts::default()
        };
        parts.joints = vec![0; 12];
        parts.weights = bytes_of(&[1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0]);
        let mesh = Arc::new(Mesh::from_parts(parts).unwrap());
        let mut moved = identity();
        moved[12] = 5.0; // joint 0 translated far right: the triangle leaves the picture
        let mut skinned = scene(Draw {
            palette: Some(0),
            ..draw()
        });
        skinned.palettes = vec![Arc::from(&moved[..])];
        let (_, _, rgba) = rasterize(&skinned, &|_| Some(mesh.clone()), &|_| None);
        assert!(
            rgba.as_chunks::<4>().0.iter().all(|pixel| pixel[3] == 0),
            "skinned off the picture"
        );
        // a palette with no joint draws the mesh rigid, where it stands
        skinned.palettes = vec![Arc::from(&moved[..8])];
        let (_, _, rgba) = rasterize(&skinned, &|_| Some(mesh.clone()), &|_| None);
        assert_eq!(
            alpha_at(&rgba, 8, 9),
            255,
            "a palette too short to hold a joint is rigid"
        );
    }

    #[test]
    fn skin_wraps_light_past_the_terminator() {
        let scene = scene(draw());
        let skin = Material {
            surface: Surface::Skin,
            wrap: 0.5,
            scatter: [1.0, 0.3, 0.2],
            ..Material::default()
        };
        // lit edge-on: plain diffuse gets nothing from the light, skin gets red
        let normal = [1.0, 0.0, 0.0];
        let plain = shade(
            &scene,
            &Material::default(),
            [1.0; 3],
            normal,
            [0.0, 0.0, -1.0],
        );
        let wrapped = shade(&scene, &skin, [1.0; 3], normal, [0.0, 0.0, -1.0]);
        assert!(wrapped[0] > plain[0] + 0.2, "red wraps round");
        assert!(
            wrapped[0] - plain[0] > wrapped[2] - plain[2],
            "more red than blue"
        );
    }

    #[test]
    fn a_zero_light_direction_shades_as_the_shader_does() {
        // the shader's `safe_normalize` turns a zero vector into +z, as `normalize` here does
        let mut zero = scene(draw());
        zero.lights[0].direction = [0.0, 0.0, 0.0];
        let lit = shade(
            &zero,
            &Material::default(),
            [1.0; 3],
            [0.0, 0.0, 1.0],
            [0.0, 0.0, 1.0],
        );
        assert!(lit.iter().all(|channel| channel.is_finite()));
    }
}
